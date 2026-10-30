use super::{drive, transfer_tests::request, IcloudClient, SavedSession};
use std::time::Duration;
use tokio::{io::AsyncWriteExt, net::TcpListener};
use wattdrive_domain::DriveError;

async fn upload_with_responses(
    responses: Vec<(u16, serde_json::Value)>,
) -> (Result<drive::Uploaded, DriveError>, usize) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server_base = base.clone();
    let mut server = tokio::spawn(async move {
        let mut sequence = vec![
            (
                200,
                serde_json::json!([{"url":format!("{server_base}/content"),"document_id":"D1"}]),
            ),
            (
                200,
                serde_json::json!({"singleFile":{"size":5,"receipt":"stable-receipt"}}),
            ),
        ];
        sequence.extend(responses);
        let mut commits = Vec::new();
        for (index, (status, body)) in sequence.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (path, request_body) = request(&mut socket).await;
            if index == 0 {
                assert!(path.ends_with("/upload/web"));
            } else if index == 1 {
                assert_eq!(path, "/content");
            } else {
                assert!(path.ends_with("/update/documents"));
                assert_eq!(request_body["document_id"], "D1");
                assert_eq!(request_body["data"]["receipt"], "stable-receipt");
                commits.push(request_body);
            }
            let body = body.to_string();
            let response = format!("HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        assert!(commits.windows(2).all(|pair| pair[0] == pair[1]));
        commits.len()
    });
    let saved = SavedSession {
        account_info: serde_json::from_value(
            serde_json::json!({"webservices":{"docws":{"url":base}}}),
        )
        .unwrap(),
        ..SavedSession::default()
    };
    let client = IcloudClient::new(Some(saved), None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sample.txt");
    tokio::fs::write(&src, b"hello").await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        drive::upload(&client, drive::ROOT_DRIVEWSID, "sample.txt", &src, 1),
    )
    .await
    .unwrap();
    let commits = match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(completed) => completed.unwrap(),
        Err(_) => {
            server.abort();
            let _cancelled = server.await;
            0
        }
    };
    (result, commits)
}

#[tokio::test]
async fn explicit_busy_retries_only_registration_with_the_same_document_and_receipt() {
    let (result, commits) = upload_with_responses(vec![
        (
            409,
            serde_json::json!({"error_code":"ZONE_BUSY","retryAfter":1}),
        ),
        (
            502,
            serde_json::json!({"serverErrorCode":"SERVER_OVERLOADED","retryAfter":1}),
        ),
        (
            200,
            serde_json::json!({"results":[{"document":{"document_id":"D1","etag":"e","size":5}}]}),
        ),
    ])
    .await;
    assert!(
        result.is_ok(),
        "a temporary busy response must not restart the whole sync"
    );
    assert_eq!(commits, 3);
}

#[tokio::test]
async fn ambiguous_registration_failure_is_not_replayed() {
    let (result, commits) =
        upload_with_responses(vec![(502, serde_json::json!({"error_code":"UNKNOWN"}))]).await;
    assert!(matches!(result, Err(DriveError::Api { status: 502, .. })));
    assert_eq!(commits, 1);
}

#[tokio::test]
async fn persistent_busy_stops_after_three_retries() {
    let (result, commits) = upload_with_responses(vec![
        (
            409,
            serde_json::json!({"error_code":"ZONE_BUSY","retryAfter":1})
        );
        4
    ])
    .await;
    assert!(matches!(result, Err(DriveError::RateLimited)));
    assert_eq!(commits, 4);
}
