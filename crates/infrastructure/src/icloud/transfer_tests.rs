//! Exercise the real HTTP upload path against a synthetic service.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{drive, IcloudClient, SavedSession};

#[derive(Default)]
struct Activity {
    content: AtomicUsize,
    content_peak: AtomicUsize,
    writes: AtomicUsize,
    writes_peak: AtomicUsize,
    slots: AtomicUsize,
}

pub(super) async fn request(socket: &mut TcpStream) -> (String, serde_json::Value) {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 4096];
        let n = socket.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let mut chunk = [0; 4096];
        let n = socket.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    let path = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned();
    let body = serde_json::from_slice(&bytes[header_end..]).unwrap_or_default();
    (path, body)
}

async fn serve(mut socket: TcpStream, base: String, active: Arc<Activity>) {
    let (path, body) = request(&mut socket).await;
    let mut status = 200;
    let response = if path.ends_with("/upload/web") {
        let id = active.slots.fetch_add(1, Ordering::SeqCst);
        serde_json::json!([{"url":format!("{base}/content"),"document_id":format!("D{id}")}])
    } else if path == "/content" {
        let count = active.content.fetch_add(1, Ordering::SeqCst) + 1;
        active.content_peak.fetch_max(count, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        active.content.fetch_sub(1, Ordering::SeqCst);
        serde_json::json!({"singleFile":{"size":5,"receipt":"r"}})
    } else {
        let count = active.writes.fetch_add(1, Ordering::SeqCst) + 1;
        active.writes_peak.fetch_max(count, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(80)).await;
        active.writes.fetch_sub(1, Ordering::SeqCst);
        if count > 1 {
            status = 409;
            serde_json::json!({"error_code":"ZONE_BUSY"})
        } else if path.ends_with("/update/documents") {
            serde_json::json!({"results":[{"document":{"document_id":body["document_id"],"etag":"e","size":5}}]})
        } else if path == "/createFolders" {
            serde_json::json!({"folders":[{"drivewsid":"FOLDER::z::new","status":"OK"}]})
        } else {
            assert_eq!(path, "/moveItemsToTrash");
            serde_json::json!({"items":[{"status":"OK"}]})
        }
    };
    let body = response.to_string();
    let headers = format!("HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(body.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn parallel_content_has_serial_cloud_mutations() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let activity = Arc::new(Activity::default());
    let task_activity = activity.clone();
    let server_base = base.clone();
    let server = tokio::spawn(async move {
        let mut handlers = tokio::task::JoinSet::new();
        // Two uploads (three requests each), plus folder creation and trash.
        for _ in 0..8 {
            let (socket, _) = listener.accept().await.unwrap();
            handlers.spawn(serve(socket, server_base.clone(), task_activity.clone()));
        }
        while let Some(result) = handlers.join_next().await {
            result.unwrap();
        }
    });
    let saved = SavedSession {
        account_info: serde_json::from_value(serde_json::json!({"webservices":{
            "docws":{"url":base},"drivews":{"url":base}
        }}))
        .unwrap(),
        ..SavedSession::default()
    };
    let client = IcloudClient::new(Some(saved), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("synthetic.txt");
    tokio::fs::write(&source, b"hello").await.unwrap();
    let (a, b, folder, trash) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            drive::upload(&client, drive::ROOT_DRIVEWSID, "a.txt", &source, 1),
            drive::upload(&client, drive::ROOT_DRIVEWSID, "b.txt", &source, 1),
            drive::create_folder(&client, drive::ROOT_DRIVEWSID, "new"),
            drive::trash(&client, "FILE::z::old", "e")
        )
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        a.is_ok() && b.is_ok() && folder.is_ok() && trash.is_ok(),
        "overlapping cloud writes were rejected"
    );
    assert_eq!(activity.content_peak.load(Ordering::SeqCst), 2);
    assert_eq!(activity.writes_peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn busy_service_errors_stop_scheduling_more_transfers() {
    for (status, body) in [
        (409, r#"{"error_code":"ZONE_BUSY"}"#),
        (
            502,
            r#"{"error_code":"WSInternalError","serverErrorCode":"SERVER_OVERLOADED","retryAfter":10}"#,
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let response = format!(
                "HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let response = reqwest::get(url).await.unwrap();
        let result = super::auth::check_status(response, "register upload").await;
        server.await.unwrap();
        assert!(
            matches!(result, Err(wattdrive_domain::DriveError::RateLimited)),
            "{result:?}"
        );
    }
}
