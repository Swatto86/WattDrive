//! iCloud Drive endpoints: listing (drivews), download/upload (docws),
//! folder creation and trash (drivews).

use std::path::Path;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use wattdrive_domain::DriveError;

use super::auth::{check_status, IcloudClient};
use super::wire::{
    CreateFoldersResponse, DocumentUpdateResponse, DriveItem, FileFlags, FileRequest,
    ItemsEnvelope, SingleFileResponse, UpdateData, UpdateDocument, UpdatePath, UploadTarget,
};

pub const DEFAULT_ZONE: &str = "com.apple.CloudDocs";
pub const ROOT_DRIVEWSID: &str = "FOLDER::com.apple.CloudDocs::root";

/// `TYPE::zone::id` → zone (default zone when the id has none).
pub fn zone_of(drivewsid: &str) -> &str {
    match drivewsid.split("::").nth(1) {
        Some(z) if !z.is_empty() => z,
        _ => DEFAULT_ZONE,
    }
}

/// `TYPE::zone::id` → id (the docws `document_id`).
pub fn doc_id_of(drivewsid: &str) -> &str {
    drivewsid.rsplit("::").next().unwrap_or(drivewsid)
}

/// The result of an upload, enough to build a `RemoteFile`.
pub struct Uploaded {
    pub drivewsid: String,
    pub etag: String,
    pub size: u64,
}

fn endpoint(client: &IcloudClient, key: &str) -> Result<String, DriveError> {
    client
        .saved_session()
        .account_info
        .webservices
        .get(key)
        .map(|w| w.url.trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| DriveError::SignInRequired(format!("no {key} endpoint in session")))
}

/// List several folders in one request. Each returned item is a folder with
/// its `items` filled in; the caller matches them back by `drivewsid`.
pub async fn list_folders(
    client: &IcloudClient,
    drivewsids: Vec<String>,
) -> Result<Vec<DriveItem>, DriveError> {
    let url = format!(
        "{}/retrieveItemDetailsInFolders",
        endpoint(client, "drivews")?
    );
    let body: Vec<serde_json::Value> = drivewsids
        .iter()
        .map(|id| json!({"drivewsid": id, "partialData": false, "includeHierarchy": false}))
        .collect();
    client
        .service_json("list folders", &|http, h| {
            http.post(&url).headers(h).json(&body)
        })
        .await
}

pub async fn list_folder(
    client: &IcloudClient,
    drivewsid: &str,
) -> Result<Vec<DriveItem>, DriveError> {
    list_folders(client, vec![drivewsid.to_string()])
        .await?
        .into_iter()
        .next()
        .map(|folder| folder.items)
        .ok_or(DriveError::Api {
            status: 200,
            message: "list folder: empty response".into(),
        })
}

pub async fn download_url(client: &IcloudClient, drivewsid: &str) -> Result<String, DriveError> {
    let url = format!(
        "{}/ws/{}/download/by_id",
        endpoint(client, "docws")?,
        zone_of(drivewsid)
    );
    let doc_id = doc_id_of(drivewsid).to_string();
    let req: FileRequest = client
        .service_json("download url", &|http, h| {
            http.get(&url)
                .headers(h)
                .query(&[("document_id", doc_id.as_str())])
        })
        .await?;
    req.data_token
        .or(req.package_token)
        .map(|t| t.url)
        .ok_or(DriveError::Api {
            status: 200,
            message: "download url: no token in response".into(),
        })
}

/// Stream a content URL to `dest`, following iCloud's non-standard 330.
pub async fn download_to(client: &IcloudClient, url: &str, dest: &Path) -> Result<(), DriveError> {
    let mut url = url.to_string();
    for _ in 0..4 {
        let u = url.clone();
        let resp = client
            .service_send(&|http, h| http.get(&u).headers(h))
            .await?;
        if resp.status().as_u16() == 330 {
            url = resp
                .headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .ok_or(DriveError::Api {
                    status: 330,
                    message: "download: 330 without Location".into(),
                })?;
            continue;
        }
        let mut resp = check_status(resp, "download").await?;
        let mut file = tokio::fs::File::create(dest).await?;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| DriveError::Network(e.to_string()))?
        {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        return Ok(());
    }
    Err(DriveError::Api {
        status: 330,
        message: "download: too many redirects".into(),
    })
}

pub async fn create_folder(
    client: &IcloudClient,
    parent_drivewsid: &str,
    name: &str,
) -> Result<DriveItem, DriveError> {
    let url = format!("{}/createFolders", endpoint(client, "drivews")?);
    let body = json!({
        "destinationDrivewsId": parent_drivewsid,
        "folders": [{
            "clientId": format!("FOLDER::UNKNOWN_ZONE::TempId-{}", uuid::Uuid::new_v4()),
            "name": name,
        }],
    });
    let resp: CreateFoldersResponse = client
        .service_json("create folder", &|http, h| {
            http.post(&url).headers(h).json(&body)
        })
        .await?;
    let folder = resp.folders.into_iter().next().ok_or(DriveError::Api {
        status: 200,
        message: "create folder: empty response".into(),
    })?;
    if folder.status != "OK" {
        return Err(DriveError::Api {
            status: 200,
            message: format!("create folder {name:?}: {}", folder.status),
        });
    }
    Ok(folder)
}

pub async fn trash(client: &IcloudClient, drivewsid: &str, etag: &str) -> Result<(), DriveError> {
    let url = format!("{}/moveItemsToTrash", endpoint(client, "drivews")?);
    let body = json!({"items": [{"drivewsid": drivewsid, "etag": etag, "clientId": drivewsid}]});
    let resp: ItemsEnvelope = client
        .service_json("trash", &|http, h| http.post(&url).headers(h).json(&body))
        .await?;
    match resp.items.first().map(|i| i.status.as_str()) {
        Some("OK") => Ok(()),
        Some("ETAG_CONFLICT") => Err(DriveError::Conflict),
        Some(other) => Err(DriveError::Api {
            status: 200,
            message: format!("trash: {other}"),
        }),
        None => Err(DriveError::Api {
            status: 200,
            message: "trash: empty response".into(),
        }),
    }
}

/// Three steps, as icloud.com does it: ask for an upload slot, POST the raw
/// bytes there, then register the document under its parent.
pub async fn upload(
    client: &IcloudClient,
    parent_drivewsid: &str,
    name: &str,
    src: &Path,
    mtime_ms: i64,
) -> Result<Uploaded, DriveError> {
    let zone = zone_of(parent_drivewsid).to_string();
    let docws = endpoint(client, "docws")?;
    let size = tokio::fs::metadata(src).await?.len();
    let content_type = content_type_for(name);

    // 1. upload slot
    let slot_url = format!("{docws}/ws/{zone}/upload/web");
    let slot_body = json!({
        "filename": name,
        "type": "FILE",
        "size": size.to_string(),
        "content_type": content_type,
    });
    let targets: Vec<UploadTarget> = client
        .service_json("upload slot", &|http, h| {
            http.post(&slot_url).headers(h).json(&slot_body)
        })
        .await?;
    let target = targets.into_iter().next().ok_or(DriveError::Api {
        status: 200,
        message: "upload slot: empty response".into(),
    })?;

    // 2. raw content
    let src_path = src.to_path_buf();
    let ct = HeaderValue::from_str(&content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let single: SingleFileResponse = client
        .service_json("upload content", &|http, mut h: HeaderMap| {
            h.insert(CONTENT_TYPE, ct.clone());
            h.insert(CONTENT_LENGTH, HeaderValue::from(size));
            let path = src_path.clone();
            let body = reqwest::Body::wrap_stream(
                futures_util::stream::once(async move { tokio::fs::File::open(path).await })
                    .map_file_stream(),
            );
            http.post(&target.url).headers(h).body(body)
        })
        .await?;
    let sf = single.single_file;

    // 3. register the document
    let update_url = format!("{docws}/ws/{zone}/update/documents");
    let parent_doc = doc_id_of(parent_drivewsid);
    let update = UpdateDocument {
        allow_conflict: true,
        btime: mtime_ms,
        command: "add_file",
        create_short_guid: true,
        data: UpdateData {
            receipt: &sf.receipt,
            reference_signature: &sf.reference_checksum,
            signature: &sf.file_checksum,
            size: sf.size,
            wrapping_key: &sf.wrapping_key,
        },
        document_id: &target.document_id,
        file_flags: FileFlags {
            is_executable: false,
            is_hidden: false,
            is_writable: true,
        },
        mtime: mtime_ms,
        path: UpdatePath {
            path: name,
            starting_document_id: parent_doc,
        },
    };
    let resp: DocumentUpdateResponse = client
        .service_json("register upload", &|http, h| {
            http.post(&update_url).headers(h).json(&update)
        })
        .await?;
    let result = resp.results.into_iter().next().ok_or(DriveError::Api {
        status: 200,
        message: "register upload: empty response".into(),
    })?;
    if result.status.status_code != 0 {
        return Err(DriveError::Api {
            status: 200,
            message: format!(
                "register upload {name:?}: {} ({})",
                result.status.error_message, result.status.status_code
            ),
        });
    }
    let doc = result.document.ok_or(DriveError::Api {
        status: 200,
        message: "register upload: no document".into(),
    })?;
    let doc_zone = if doc.zone.is_empty() { zone } else { doc.zone };
    Ok(Uploaded {
        drivewsid: format!("FILE::{doc_zone}::{}", doc.document_id),
        etag: doc.etag,
        size: if doc.size > 0 { doc.size } else { size },
    })
}

/// docws insists on a content type; guess from the extension, text/plain
/// otherwise (what the web app sends for unknown files).
fn content_type_for(name: &str) -> String {
    mime_guess::from_path(name)
        .first_raw()
        .unwrap_or("text/plain")
        .to_string()
}

/// Adapter so a `File` open future can be turned into a byte stream body.
trait FileStreamExt {
    fn map_file_stream(
        self,
    ) -> futures_util::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>>;
}

impl<S> FileStreamExt for S
where
    S: futures_util::Stream<Item = std::io::Result<tokio::fs::File>> + Send + 'static,
{
    fn map_file_stream(
        self,
    ) -> futures_util::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>> {
        use futures_util::TryStreamExt;
        Box::pin(self.map_ok(tokio_util::io::ReaderStream::new).try_flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drivewsid_parts() {
        assert_eq!(
            zone_of("FILE::com.apple.CloudDocs::ABC"),
            "com.apple.CloudDocs"
        );
        assert_eq!(
            zone_of("FOLDER::com.apple.Pages::documents"),
            "com.apple.Pages"
        );
        assert_eq!(zone_of(ROOT_DRIVEWSID), DEFAULT_ZONE);
        assert_eq!(zone_of("weird"), DEFAULT_ZONE);
        assert_eq!(doc_id_of(ROOT_DRIVEWSID), "root");
        assert_eq!(doc_id_of("FILE::z::ABC"), "ABC");
        assert_eq!(doc_id_of("plain"), "plain");
    }

    #[test]
    fn content_type_guesses_or_falls_back() {
        assert_eq!(content_type_for("a.png"), "image/png");
        assert_eq!(content_type_for("doc.pdf"), "application/pdf");
        assert_eq!(content_type_for("Makefile"), "text/plain");
        assert_eq!(content_type_for("notes.md"), "text/markdown");
    }
}
