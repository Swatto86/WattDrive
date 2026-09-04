//! `RemoteDrive` implementation over the iCloud client: maps Apple's item
//! shapes onto the domain model (NFC names, ms timestamps, typed ids).

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use time::format_description::well_known::Rfc3339;
use unicode_normalization::UnicodeNormalization;
use wattdrive_domain::{DriveError, RemoteChild, RemoteDrive, RemoteFile, RemoteId, RemoteNode};

use super::auth::IcloudClient;
use super::drive::{self, ROOT_DRIVEWSID};
use super::wire::DriveItem;

pub struct IcloudDrive {
    client: Arc<IcloudClient>,
}

impl IcloudDrive {
    pub fn new(client: Arc<IcloudClient>) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &Arc<IcloudClient> {
        &self.client
    }
}

/// iCloud reports names in NFD; Linux users type NFC. Normalise so the same
/// file has the same path on both sides.
fn nfc(s: &str) -> String {
    s.nfc().collect()
}

fn parse_ms(iso: &str) -> i64 {
    if iso.is_empty() {
        return 0;
    }
    match time::OffsetDateTime::parse(iso, &Rfc3339) {
        Ok(t) => i64::try_from(t.unix_timestamp_nanos() / 1_000_000).unwrap_or(0),
        Err(e) => {
            tracing::warn!("unparseable iCloud date {iso:?}: {e}");
            0
        }
    }
}

pub(crate) fn to_child(item: DriveItem) -> Option<RemoteChild> {
    let name = nfc(&item.full_name());
    if name.is_empty() {
        return None;
    }
    let node = if item.is_folder() {
        RemoteNode::Folder {
            id: RemoteId(item.drivewsid),
            etag: item.etag,
        }
    } else if item.is_file() {
        RemoteNode::File(RemoteFile {
            id: RemoteId(item.drivewsid),
            etag: item.etag,
            size: item.size,
            modified_ms: parse_ms(&item.date_modified),
        })
    } else {
        tracing::debug!("skipping iCloud item {name:?} of type {:?}", item.kind);
        return None;
    };
    Some(RemoteChild { name, node })
}

#[async_trait]
impl RemoteDrive for IcloudDrive {
    fn root(&self) -> RemoteId {
        RemoteId(ROOT_DRIVEWSID.to_string())
    }

    async fn list_children(&self, folder: &RemoteId) -> Result<Vec<RemoteChild>, DriveError> {
        let items = drive::list_folder(&self.client, folder.as_str()).await?;
        Ok(items.into_iter().filter_map(to_child).collect())
    }

    async fn download(&self, file: &RemoteFile, dest: &Path) -> Result<(), DriveError> {
        let url = drive::download_url(&self.client, file.id.as_str()).await?;
        drive::download_to(&self.client, &url, dest).await
    }

    async fn upload(
        &self,
        parent: &RemoteId,
        name: &str,
        src: &Path,
        mtime_ms: i64,
    ) -> Result<RemoteFile, DriveError> {
        let up = drive::upload(&self.client, parent.as_str(), name, src, mtime_ms).await?;
        Ok(RemoteFile {
            id: RemoteId(up.drivewsid),
            etag: up.etag,
            size: up.size,
            modified_ms: mtime_ms,
        })
    }

    async fn create_folder(&self, parent: &RemoteId, name: &str) -> Result<RemoteId, DriveError> {
        let folder = drive::create_folder(&self.client, parent.as_str(), name).await?;
        Ok(RemoteId(folder.drivewsid))
    }

    async fn trash(&self, id: &RemoteId, etag: &str) -> Result<(), DriveError> {
        drive::trash(&self.client, id.as_str(), etag).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, name: &str, ext: &str) -> DriveItem {
        DriveItem {
            drivewsid: format!("{kind}::com.apple.CloudDocs::X"),
            name: name.into(),
            extension: ext.into(),
            etag: "e".into(),
            kind: kind.into(),
            size: 42,
            date_modified: "2026-08-30T10:11:12Z".into(),
            ..DriveItem::default()
        }
    }

    #[test]
    fn maps_files_folders_and_normalises_names() {
        let f = to_child(item("FILE", "r\u{0065}\u{0301}sum\u{0065}\u{0301}", "pdf")).unwrap();
        assert_eq!(f.name, "résumé.pdf", "NFD from iCloud becomes NFC");
        match f.node {
            RemoteNode::File(rf) => {
                assert_eq!(rf.size, 42);
                assert_eq!(rf.modified_ms, 1_788_084_672_000);
                assert_eq!(rf.id.as_str(), "FILE::com.apple.CloudDocs::X");
            }
            other => panic!("{other:?}"),
        }
        let d = to_child(item("FOLDER", "Documents", "")).unwrap();
        assert!(matches!(d.node, RemoteNode::Folder { .. }));
        assert_eq!(d.name, "Documents");
        assert!(
            to_child(item("ALIAS", "x", "")).is_none(),
            "unknown kinds are skipped"
        );
        assert!(
            to_child(item("FILE", "", "")).is_none(),
            "nameless items are skipped"
        );
    }

    #[test]
    fn dates_parse_with_and_without_fraction() {
        assert_eq!(parse_ms("2026-08-30T10:11:12Z"), 1_788_084_672_000);
        assert_eq!(parse_ms("2026-08-30T10:11:12.345Z"), 1_788_084_672_345);
        assert_eq!(parse_ms(""), 0);
        assert_eq!(parse_ms("garbage"), 0);
    }
}
