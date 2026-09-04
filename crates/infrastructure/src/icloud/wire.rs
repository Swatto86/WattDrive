//! JSON shapes of the iCloud endpoints we call. Only the fields we read are
//! declared; serde ignores the rest. Kept together so a change in Apple's
//! payloads is a one-file fix, and each shape has a decode test against a
//! captured-style payload.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// `POST /signin/init` response.
#[derive(Debug, Deserialize)]
pub struct SrpInitResponse {
    pub iteration: u32,
    pub salt: String,
    pub protocol: String,
    pub b: String,
    pub c: String,
}

/// `/accountLogin` and `/validate` response (the subset we use).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    #[serde(rename = "dsInfo", default)]
    pub ds_info: Option<DsInfo>,
    #[serde(default)]
    pub webservices: HashMap<String, WebService>,
    #[serde(rename = "hsaChallengeRequired", default)]
    pub hsa_challenge_required: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DsInfo {
    #[serde(rename = "hsaVersion", default)]
    pub hsa_version: i32,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WebService {
    #[serde(rename = "pcsRequired", default)]
    pub pcs_required: bool,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub status: String,
}

/// `GET /appleauth/auth` after a 409: where the second factor can go.
#[derive(Debug, Default, Deserialize)]
pub struct AuthState {
    #[serde(rename = "trustedPhoneNumbers", default)]
    pub trusted_phone_numbers: Vec<TrustedPhoneNumber>,
    #[serde(rename = "trustedPhoneNumber", default)]
    pub trusted_phone_number: Option<TrustedPhoneNumber>,
    #[serde(rename = "noTrustedDevices", default)]
    pub no_trusted_devices: bool,
    /// Some accounts nest the whole payload under this key.
    #[serde(rename = "phoneNumberVerification", default)]
    pub phone_number_verification: Option<Box<AuthState>>,
}

impl AuthState {
    /// Flatten the optional envelope and the singular fallback into one list.
    pub fn phones(self) -> (Vec<TrustedPhoneNumber>, bool) {
        let inner = match self.phone_number_verification {
            Some(inner) if self.trusted_phone_numbers.is_empty() => *inner,
            _ => self,
        };
        let mut phones = inner.trusted_phone_numbers;
        if phones.is_empty() {
            phones.extend(inner.trusted_phone_number);
        }
        (phones, inner.no_trusted_devices)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrustedPhoneNumber {
    pub id: i64,
    #[serde(rename = "numberWithDialCode", default)]
    pub number_with_dial_code: String,
    #[serde(rename = "pushMode", default)]
    pub push_mode: String,
}

/// `POST /requestPCS` response.
#[derive(Debug, Default, Deserialize)]
pub struct PcsResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
}

/// An item as `retrieveItemDetailsInFolders` / `createFolders` /
/// `moveItemsToTrash` report it.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct DriveItem {
    #[serde(default)]
    pub drivewsid: String,
    #[serde(default)]
    pub docwsid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub extension: String,
    #[serde(default)]
    pub etag: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub size: u64,
    #[serde(rename = "dateModified", default)]
    pub date_modified: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub items: Vec<DriveItem>,
}

impl DriveItem {
    pub fn is_folder(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "FOLDER" | "APP_CONTAINER" | "APP_LIBRARY"
        )
    }

    pub fn is_file(&self) -> bool {
        self.kind == "FILE"
    }

    /// `name` + `.` + `extension` for files; folders carry no extension.
    pub fn full_name(&self) -> String {
        if self.extension.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.name, self.extension)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ItemsEnvelope {
    #[serde(default)]
    pub items: Vec<DriveItem>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFoldersResponse {
    #[serde(default)]
    pub folders: Vec<DriveItem>,
}

/// `GET /ws/{zone}/download/by_id`.
#[derive(Debug, Deserialize)]
pub struct FileRequest {
    #[serde(default)]
    pub data_token: Option<FileToken>,
    #[serde(default)]
    pub package_token: Option<FileToken>,
}

#[derive(Debug, Deserialize)]
pub struct FileToken {
    pub url: String,
}

/// `POST /ws/{zone}/upload/web` — one entry per requested file.
#[derive(Debug, Deserialize)]
pub struct UploadTarget {
    pub url: String,
    pub document_id: String,
}

/// Body of the raw content POST to the upload URL.
#[derive(Debug, Deserialize)]
pub struct SingleFileResponse {
    #[serde(rename = "singleFile")]
    pub single_file: SingleFileInfo,
}

#[derive(Debug, Deserialize)]
pub struct SingleFileInfo {
    #[serde(rename = "referenceChecksum", default)]
    pub reference_checksum: String,
    #[serde(default)]
    pub size: u64,
    #[serde(rename = "fileChecksum", default)]
    pub file_checksum: String,
    #[serde(rename = "wrappingKey", default)]
    pub wrapping_key: String,
    #[serde(default)]
    pub receipt: String,
}

/// `POST /ws/{zone}/update/documents` request.
#[derive(Debug, Serialize)]
pub struct UpdateDocument<'a> {
    pub allow_conflict: bool,
    pub btime: i64,
    pub command: &'static str,
    pub create_short_guid: bool,
    pub data: UpdateData<'a>,
    pub document_id: &'a str,
    pub file_flags: FileFlags,
    pub mtime: i64,
    pub path: UpdatePath<'a>,
}

#[derive(Debug, Serialize)]
pub struct UpdateData<'a> {
    pub receipt: &'a str,
    pub reference_signature: &'a str,
    pub signature: &'a str,
    pub size: u64,
    pub wrapping_key: &'a str,
}

#[derive(Debug, Serialize)]
pub struct FileFlags {
    pub is_executable: bool,
    pub is_hidden: bool,
    pub is_writable: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdatePath<'a> {
    pub path: &'a str,
    pub starting_document_id: &'a str,
}

/// `update/documents` response.
#[derive(Debug, Deserialize)]
pub struct DocumentUpdateResponse {
    #[serde(default)]
    pub results: Vec<DocumentUpdateResult>,
}

#[derive(Debug, Deserialize)]
pub struct DocumentUpdateResult {
    #[serde(default)]
    pub status: OperationStatus,
    #[serde(default)]
    pub document: Option<Document>,
}

#[derive(Debug, Default, Deserialize)]
pub struct OperationStatus {
    #[serde(default)]
    pub status_code: i64,
    #[serde(default)]
    pub error_message: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub document_id: String,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub zone: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_listing_decodes_files_folders_and_app_libraries() {
        let json = r#"[{
          "drivewsid": "FOLDER::com.apple.CloudDocs::root", "etag": "abc", "type": "FOLDER",
          "name": "root", "numberOfItems": 3,
          "items": [
            {"drivewsid":"FILE::com.apple.CloudDocs::11", "docwsid":"11", "etag":"e1", "type":"FILE",
             "name":"todo", "extension":"md", "size":1234, "dateModified":"2026-08-30T10:11:12Z", "parentId":"FOLDER::com.apple.CloudDocs::root"},
            {"drivewsid":"FOLDER::com.apple.CloudDocs::22", "docwsid":"22", "etag":"e2", "type":"FOLDER", "name":"Documents", "directChildrenCount": 4},
            {"drivewsid":"FOLDER::com.apple.Pages::documents", "etag":"e3", "type":"APP_LIBRARY", "name":"Pages", "zone":"com.apple.Pages"},
            {"drivewsid":"FILE::com.apple.CloudDocs::33", "etag":"e4", "type":"FILE", "name":"Makefile", "extension":"", "size":0}
          ]}]"#;
        let items: Vec<DriveItem> = serde_json::from_str(json).unwrap();
        let root = &items[0];
        assert_eq!(root.items.len(), 4);
        assert!(root.items[0].is_file());
        assert_eq!(root.items[0].full_name(), "todo.md");
        assert_eq!(root.items[0].size, 1234);
        assert_eq!(root.items[0].date_modified, "2026-08-30T10:11:12Z");
        assert!(root.items[1].is_folder());
        assert_eq!(root.items[1].full_name(), "Documents");
        assert!(root.items[2].is_folder(), "app libraries list as folders");
        assert_eq!(root.items[3].full_name(), "Makefile");
    }

    #[test]
    fn size_given_as_a_string_is_rejected_not_misread() {
        // Apple's docws endpoints send size as a string; drivews sends a number.
        // The listing decoder is for drivews, so a string must fail loudly
        // rather than silently become 0 — the adapter would then treat every
        // file as empty and "changed".
        let json = r#"{"drivewsid":"x","type":"FILE","name":"a","size":"12"}"#;
        assert!(serde_json::from_str::<DriveItem>(json).is_err());
    }

    #[test]
    fn account_info_reads_webservices_and_pcs_flag() {
        let json = r#"{"dsInfo":{"hsaVersion":2,"dsid":"1"},
          "hsaChallengeRequired":false,
          "webservices":{
            "drivews":{"pcsRequired":true,"url":"https://p123-drivews.icloud.com:443","status":"active"},
            "docws":{"url":"https://p123-docws.icloud.com:443","status":"active"}}}"#;
        let info: AccountInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.ds_info.unwrap().hsa_version, 2);
        assert!(info.webservices["drivews"].pcs_required);
        assert!(!info.webservices["docws"].pcs_required);
        assert_eq!(
            info.webservices["docws"].url,
            "https://p123-docws.icloud.com:443"
        );
    }

    #[test]
    fn auth_state_flattens_envelope_and_singular_phone() {
        let nested = r#"{"phoneNumberVerification":{"trustedPhoneNumber":{"id":7,"numberWithDialCode":"+44 •••• ••1234","pushMode":"sms"},"noTrustedDevices":false}}"#;
        let (phones, no_devices) = serde_json::from_str::<AuthState>(nested).unwrap().phones();
        assert_eq!(phones.len(), 1);
        assert_eq!(phones[0].id, 7);
        assert!(!no_devices);

        let flat = r#"{"trustedPhoneNumbers":[{"id":1,"numberWithDialCode":"+1","pushMode":"sms"},{"id":2,"numberWithDialCode":"+2","pushMode":"voice"}],"noTrustedDevices":true}"#;
        let (phones, no_devices) = serde_json::from_str::<AuthState>(flat).unwrap().phones();
        assert_eq!(phones.iter().map(|p| p.id).collect::<Vec<_>>(), vec![1, 2]);
        assert!(no_devices);
    }

    #[test]
    fn upload_and_download_shapes_decode() {
        let targets: Vec<UploadTarget> =
            serde_json::from_str(r#"[{"url":"https://up.example/x","document_id":"D1"}]"#).unwrap();
        assert_eq!(targets[0].document_id, "D1");
        let single: SingleFileResponse = serde_json::from_str(
            r#"{"singleFile":{"referenceChecksum":"r","size":5,"fileChecksum":"f","wrappingKey":"w","receipt":"rc"}}"#,
        )
        .unwrap();
        assert_eq!(single.single_file.size, 5);
        assert_eq!(single.single_file.receipt, "rc");
        let dl: FileRequest = serde_json::from_str(
            r#"{"document_id":"D1","data_token":{"url":"https://cvws.example/dl?x=1","token":"t"}}"#,
        )
        .unwrap();
        assert_eq!(dl.data_token.unwrap().url, "https://cvws.example/dl?x=1");
        let upd: DocumentUpdateResponse = serde_json::from_str(
            r#"{"status":{"status_code":0},"results":[{"status":{"status_code":0,"error_message":""},"operation_id":null,
                 "document":{"document_id":"D1","item_id":"I1","etag":"e9","size":5,"zone":"com.apple.CloudDocs","type":"FILE","name":"a"}}]}"#,
        )
        .unwrap();
        assert_eq!(upd.results[0].document.as_ref().unwrap().etag, "e9");
    }

    #[test]
    fn update_document_request_serialises_with_apples_field_names() {
        let req = UpdateDocument {
            allow_conflict: true,
            btime: 1_000,
            command: "add_file",
            create_short_guid: true,
            data: UpdateData {
                receipt: "r",
                reference_signature: "rs",
                signature: "s",
                size: 3,
                wrapping_key: "w",
            },
            document_id: "D1",
            file_flags: FileFlags {
                is_executable: false,
                is_hidden: false,
                is_writable: true,
            },
            mtime: 2_000,
            path: UpdatePath {
                path: "a.txt",
                starting_document_id: "root",
            },
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["command"], "add_file");
        assert_eq!(v["data"]["reference_signature"], "rs");
        assert_eq!(v["path"]["starting_document_id"], "root");
        assert_eq!(v["file_flags"]["is_writable"], true);
    }
}
