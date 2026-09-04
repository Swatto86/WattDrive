//! Sign-in, second factor, session trust and the authenticated request path
//! with silent re-authentication. Mirrors what icloud.com's own sign-in frame
//! does against idmsa.apple.com and setup.icloud.com.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use reqwest::header::HeaderMap;
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;
use wattdrive_domain::DriveError;

use super::session::{
    common_headers, SavedSession, Session, AUTH_URL, PCS_DOCUMENTS_COOKIE, SETUP_URL, USER_AGENT,
};
use super::srp::{derive_password, SrpClient};
use super::wire::{AccountInfo, AuthState, PcsResponse, SrpInitResponse};

#[derive(Clone)]
pub struct Credentials {
    pub apple_id: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("apple_id", &self.apple_id)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPhone {
    pub id: i64,
    pub number: String,
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInStep {
    SignedIn,
    /// Apple pushed a code to the trusted devices; `phones` can receive an SMS
    /// instead.
    NeedsTwoFactor {
        phones: Vec<TrustedPhone>,
    },
}

pub type SessionHook = Arc<dyn Fn(&SavedSession) + Send + Sync>;
pub type ProgressHook = Arc<dyn Fn(&str) + Send + Sync>;

/// How long to wait for an Advanced Data Protection approval on a trusted
/// device before giving up.
const PCS_ATTEMPTS: u32 = 30;
const PCS_POLL: Duration = Duration::from_secs(10);

pub struct IcloudClient {
    http: Client,
    session: Mutex<Session>,
    credentials: Mutex<Option<Credentials>>,
    on_session_changed: Mutex<Option<SessionHook>>,
    on_progress: Mutex<Option<ProgressHook>>,
    /// Serialises re-authentication so concurrent 401s sign in once.
    reauth: tokio::sync::Mutex<()>,
}

fn net(e: reqwest::Error) -> DriveError {
    DriveError::Network(e.to_string())
}

impl IcloudClient {
    pub fn new(
        saved: Option<SavedSession>,
        credentials: Option<Credentials>,
    ) -> Result<Self, DriveError> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(net)?;
        Ok(Self {
            http,
            session: Mutex::new(Session::new(saved)),
            credentials: Mutex::new(credentials),
            on_session_changed: Mutex::new(None),
            on_progress: Mutex::new(None),
            reauth: tokio::sync::Mutex::new(()),
        })
    }

    pub fn set_session_hook(&self, hook: SessionHook) {
        *lock(&self.on_session_changed) = Some(hook);
    }

    pub fn set_progress_hook(&self, hook: ProgressHook) {
        *lock(&self.on_progress) = Some(hook);
    }

    fn progress(&self, msg: &str) {
        if let Some(h) = lock(&self.on_progress).as_ref() {
            h(msg);
        }
    }

    fn notify_saved(&self) {
        let saved = self.saved_session();
        if let Some(h) = lock(&self.on_session_changed).as_ref() {
            h(&saved);
        }
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        lock(&self.session)
    }

    pub fn saved_session(&self) -> SavedSession {
        self.session().saved.clone()
    }

    pub fn has_credentials(&self) -> bool {
        lock(&self.credentials).is_some()
    }

    pub fn credentials(&self) -> Option<Credentials> {
        lock(&self.credentials).clone()
    }

    /// Forget everything: credentials and the whole session.
    pub fn sign_out(&self) {
        *lock(&self.credentials) = None;
        *self.session() = Session::new(None);
    }

    pub fn apple_id(&self) -> Option<String> {
        lock(&self.credentials).as_ref().map(|c| c.apple_id.clone())
    }

    /// Cookies + service endpoints on hand: drive calls can be attempted.
    pub fn is_usable(&self) -> bool {
        self.session().saved.looks_usable()
    }

    /// Send, then absorb Apple's session headers/cookies from the response.
    async fn send(&self, req: RequestBuilder) -> Result<Response, DriveError> {
        let before = self.session().cookie_header();
        let resp = req.send().await.map_err(net)?;
        let changed = {
            let mut s = self.session();
            s.absorb(resp.headers());
            s.cookie_header() != before
        };
        if changed {
            self.notify_saved();
        }
        Ok(resp)
    }

    // ------------------------------------------------------------------ SRP

    /// Full sign-in with Apple ID + password. Ends signed in, or waiting for a
    /// second factor (`submit_code` / `submit_sms_code` finish the job).
    pub async fn sign_in(&self, creds: Credentials) -> Result<SignInStep, DriveError> {
        let apple_id = creds.apple_id.trim().to_lowercase();
        let password = creds.password.clone();
        *lock(&self.credentials) = Some(Credentials {
            apple_id: apple_id.clone(),
            password: password.clone(),
        });
        self.session().needs_2fa = false;

        self.auth_start().await?;
        self.auth_federate(&apple_id).await?;

        let srp = SrpClient::new();
        let init = self.auth_srp_init(&srp, &apple_id).await?;
        let b64 = base64::engine::general_purpose::STANDARD;
        let salt = b64
            .decode(&init.salt)
            .map_err(|e| DriveError::Other(format!("bad salt from Apple: {e}")))?;
        let server_b = b64
            .decode(&init.b)
            .map_err(|e| DriveError::Other(format!("bad B from Apple: {e}")))?;
        let derived = derive_password(&password, &salt, init.iteration, &init.protocol)
            .map_err(|e| DriveError::Other(e.to_string()))?;
        let proof = srp
            .process_challenge(apple_id.as_bytes(), &derived, &salt, &server_b)
            .map_err(|e| DriveError::Other(e.to_string()))?;

        let (status, body) = self
            .auth_srp_complete(
                &apple_id,
                &b64.encode(&proof.m1),
                &b64.encode(&proof.m2),
                &init.c,
            )
            .await?;
        match status.as_u16() {
            200 => {
                self.finish_sign_in().await?;
                Ok(SignInStep::SignedIn)
            }
            409 => {
                self.session().needs_2fa = true;
                // iOS 26.4+ no longer pushes on the 409 itself; ask explicitly.
                if let Err(e) = self.request_push().await {
                    tracing::warn!("2FA push request failed: {e}");
                }
                let phones = self.trusted_phones().await.unwrap_or_default();
                Ok(SignInStep::NeedsTwoFactor { phones })
            }
            412 => {
                self.auth_post_empty("/repair/complete").await?;
                self.finish_sign_in().await?;
                Ok(SignInStep::SignedIn)
            }
            401 | 403 => Err(DriveError::SignInRequired(
                "Incorrect Apple ID or password.".into(),
            )),
            s => Err(DriveError::Api {
                status: s,
                message: format!("sign-in failed: {}", snippet(&body)),
            }),
        }
    }

    async fn auth_start(&self) -> Result<(), DriveError> {
        let (frame, client_id) = {
            let s = self.session();
            (format!("auth-{}", s.frame_id), s.saved.client_id.clone())
        };
        let resp = self
            .send(
                self.http
                    .get(format!("{AUTH_URL}/authorize/signin"))
                    .header("Accept", "*/*")
                    .query(&[
                        ("frame_id", frame.as_str()),
                        ("language", "en_US"),
                        ("skVersion", "7"),
                        ("iframeId", frame.as_str()),
                        ("client_id", client_id.as_str()),
                        ("redirect_uri", "https://www.icloud.com"),
                        ("response_type", "code"),
                        ("response_mode", "web_message"),
                        ("state", frame.as_str()),
                        ("authVersion", "latest"),
                    ]),
            )
            .await?;
        expect_ok(resp, "auth start").await
    }

    async fn auth_federate(&self, apple_id: &str) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .post(format!("{AUTH_URL}/federate"))
                    .query(&[("isRememberMeEnabled", "true")])
                    .headers(headers)
                    .json(&json!({"accountName": apple_id, "rememberMe": true})),
            )
            .await?;
        expect_ok(resp, "federate").await
    }

    async fn auth_srp_init(
        &self,
        srp: &SrpClient,
        apple_id: &str,
    ) -> Result<SrpInitResponse, DriveError> {
        let headers = self.session().auth_headers();
        let a = base64::engine::general_purpose::STANDARD.encode(srp.public_a());
        let resp = self
            .send(
                self.http
                    .post(format!("{AUTH_URL}/signin/init"))
                    .headers(headers)
                    .json(&json!({
                        "a": a,
                        "accountName": apple_id,
                        "protocols": ["s2k", "s2k_fo"],
                    })),
            )
            .await?;
        decode_json(resp, "SRP init").await
    }

    async fn auth_srp_complete(
        &self,
        apple_id: &str,
        m1: &str,
        m2: &str,
        c: &str,
    ) -> Result<(StatusCode, String), DriveError> {
        let (headers, trust_token) = {
            let s = self.session();
            (s.auth_headers(), s.saved.trust_token.clone())
        };
        let trust_tokens: Vec<&str> = if trust_token.is_empty() {
            vec![]
        } else {
            vec![trust_token.as_str()]
        };
        let resp = self
            .send(
                self.http
                    .post(format!("{AUTH_URL}/signin/complete"))
                    .query(&[("isRememberMeEnabled", "true")])
                    .headers(headers)
                    .json(&json!({
                        "accountName": apple_id,
                        "m1": m1,
                        "m2": m2,
                        "c": c,
                        "rememberMe": true,
                        "trustTokens": trust_tokens,
                    })),
            )
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok((status, body))
    }

    async fn auth_post_empty(&self, path: &str) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .post(format!("{AUTH_URL}{path}"))
                    .headers(headers)
                    .json(&json!({})),
            )
            .await?;
        expect_ok(resp, path).await
    }

    // ------------------------------------------------------------------ 2FA

    async fn request_push(&self) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .put(format!("{AUTH_URL}/verify/trusteddevice/securitycode"))
                    .headers(headers),
            )
            .await?;
        expect_ok(resp, "request push").await
    }

    pub async fn trusted_phones(&self) -> Result<Vec<TrustedPhone>, DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .get(AUTH_URL)
                    .headers(headers)
                    .header("Content-Length", "0"),
            )
            .await?;
        let state: AuthState = decode_json(resp, "auth state").await?;
        let (phones, _) = state.phones();
        Ok(phones
            .into_iter()
            .map(|p| TrustedPhone {
                id: p.id,
                number: p.number_with_dial_code,
                mode: if p.push_mode.is_empty() {
                    "sms".into()
                } else {
                    p.push_mode
                },
            })
            .collect())
    }

    /// Code from the trusted-device prompt.
    pub async fn submit_code(&self, code: &str) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .post(format!("{AUTH_URL}/verify/trusteddevice/securitycode"))
                    .headers(headers)
                    .json(&json!({"securityCode": {"code": code.trim()}})),
            )
            .await?;
        self.accept_code_response(resp).await?;
        self.trust_and_finish().await
    }

    pub async fn request_sms(&self, phone_id: i64) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .put(format!("{AUTH_URL}/verify/phone"))
                    .headers(headers)
                    .json(&json!({"phoneNumber": {"id": phone_id}, "mode": "sms"})),
            )
            .await?;
        expect_ok(resp, "request SMS").await
    }

    pub async fn submit_sms_code(&self, code: &str, phone_id: i64) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .post(format!("{AUTH_URL}/verify/phone/securitycode"))
                    .headers(headers)
                    .json(&json!({
                        "securityCode": {"code": code.trim()},
                        "phoneNumber": {"id": phone_id},
                        "mode": "sms",
                    })),
            )
            .await?;
        self.accept_code_response(resp).await?;
        self.trust_and_finish().await
    }

    /// Since mid-2026 idmsa answers 409 even to an accepted code, but still
    /// issues the session token. Token issuance is the ground truth.
    async fn accept_code_response(&self, resp: Response) -> Result<(), DriveError> {
        let status = resp.status();
        let token_issued = resp.headers().contains_key("X-Apple-Session-Token");
        if status.is_success() || (status.as_u16() == 409 && token_issued) {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(match status.as_u16() {
            400 | 401 | 403 | 409 => DriveError::SignInRequired(
                "That verification code was not accepted. Try again.".into(),
            ),
            s => DriveError::Api {
                status: s,
                message: snippet(&body),
            },
        })
    }

    async fn trust_and_finish(&self) -> Result<(), DriveError> {
        let headers = self.session().auth_headers();
        let resp = self
            .send(
                self.http
                    .get(format!("{AUTH_URL}/2sv/trust"))
                    .headers(headers)
                    .header("Content-Length", "0"),
            )
            .await?;
        expect_ok(resp, "trust session").await?;
        self.finish_sign_in().await
    }

    async fn finish_sign_in(&self) -> Result<(), DriveError> {
        self.account_login().await?;
        self.ensure_pcs_cookies().await?;
        self.session().needs_2fa = false;
        self.notify_saved();
        Ok(())
    }

    // ---------------------------------------------------------------- setup

    async fn account_login(&self) -> Result<(), DriveError> {
        let (country, token, trust) = {
            let s = self.session();
            (
                s.saved.account_country.clone(),
                s.saved.session_token.clone(),
                s.saved.trust_token.clone(),
            )
        };
        let resp = self
            .send(
                self.http
                    .post(format!("{SETUP_URL}/accountLogin"))
                    .headers(common_headers())
                    .json(&json!({
                        "accountCountryCode": country,
                        "dsWebAuthToken": token,
                        "extended_login": true,
                        "trustToken": trust,
                    })),
            )
            .await?;
        let info: AccountInfo = decode_json(resp, "account login").await?;
        self.session().saved.account_info = info;
        Ok(())
    }

    /// Check the saved cookies still work; refreshes the service endpoints.
    pub async fn validate_session(&self) -> Result<(), DriveError> {
        let headers = self.session().service_headers();
        let resp = self
            .send(
                self.http
                    .post(format!("{SETUP_URL}/validate"))
                    .headers(headers)
                    .header("Content-Length", "0"),
            )
            .await?;
        let info: AccountInfo = decode_json(resp, "validate session").await?;
        let mut s = self.session();
        s.saved.account_info = info;
        s.needs_2fa = false;
        Ok(())
    }

    /// Advanced Data Protection accounts need per-service "PCS" cookies, which
    /// Apple only issues after the user approves on a trusted device.
    async fn ensure_pcs_cookies(&self) -> Result<(), DriveError> {
        let needed = {
            let s = self.session();
            s.saved
                .account_info
                .webservices
                .get("drivews")
                .is_some_and(|w| w.pcs_required)
                && !s.has_cookie(PCS_DOCUMENTS_COOKIE)
        };
        if !needed {
            return Ok(());
        }
        tracing::info!("Advanced Data Protection detected; requesting PCS cookies for Drive");
        for attempt in 1..=PCS_ATTEMPTS {
            let headers = self.session().service_headers();
            let resp = self
                .send(
                    self.http
                        .post(format!("{SETUP_URL}/requestPCS"))
                        .headers(headers)
                        .json(&json!({"appName": "iclouddrive", "derivedFromUserAction": true})),
                )
                .await?;
            let pcs: PcsResponse = decode_json(resp, "request PCS").await.unwrap_or_default();
            let have_cookie = self.session().has_cookie(PCS_DOCUMENTS_COOKIE);
            if pcs.status == "success" && have_cookie {
                self.notify_saved();
                return Ok(());
            }
            self.progress(
                "Approve iCloud web access for WattDrive on your iPhone, iPad or Mac \
                 (Advanced Data Protection is on).",
            );
            tracing::info!(
                "waiting for ADP approval ({attempt}/{PCS_ATTEMPTS}): {}",
                pcs.message
            );
            tokio::time::sleep(PCS_POLL).await;
        }
        Err(DriveError::SignInRequired(
            "Timed out waiting for approval on a trusted device.".into(),
        ))
    }

    // ------------------------------------------------- authenticated calls

    /// Make sure drive calls can be attempted; signs in silently if needed.
    pub async fn ensure_ready(&self) -> Result<(), DriveError> {
        if self.is_usable() {
            return Ok(());
        }
        self.reauthenticate().await
    }

    async fn reauthenticate(&self) -> Result<(), DriveError> {
        let _serialised = self.reauth.lock().await;
        let creds = lock(&self.credentials)
            .clone()
            .ok_or_else(|| DriveError::SignInRequired("Not signed in.".into()))?;
        let has_cookies = !self.session().saved.cookies.is_empty();
        if has_cookies && self.validate_session().await.is_ok() {
            self.ensure_pcs_cookies().await?;
            self.notify_saved();
            return Ok(());
        }
        self.session().saved.cookies.clear();
        match self.sign_in(creds).await? {
            SignInStep::SignedIn => Ok(()),
            SignInStep::NeedsTwoFactor { .. } => Err(DriveError::SignInRequired(
                "Two-factor verification is needed — open WattDrive and enter the code.".into(),
            )),
        }
    }

    /// Send a request to a service host with the session cookies, signing in
    /// again once if the session is rejected. Status is NOT checked here so
    /// callers can handle odd codes (e.g. 330 on downloads).
    pub async fn service_send(
        &self,
        build: &(dyn Fn(&Client, HeaderMap) -> RequestBuilder + Sync),
    ) -> Result<Response, DriveError> {
        let headers = self.session().service_headers();
        let mut resp = self.send(build(&self.http, headers)).await?;
        if matches!(resp.status().as_u16(), 401 | 421 | 423 | 450) {
            tracing::info!(
                "iCloud rejected the session ({}); re-authenticating",
                resp.status()
            );
            self.reauthenticate().await?;
            let headers = self.session().service_headers();
            resp = self.send(build(&self.http, headers)).await?;
        }
        Ok(resp)
    }

    /// `service_send` + status check + JSON decode.
    pub async fn service_json<T: DeserializeOwned>(
        &self,
        what: &str,
        build: &(dyn Fn(&Client, HeaderMap) -> RequestBuilder + Sync),
    ) -> Result<T, DriveError> {
        let resp = self.service_send(build).await?;
        decode_json(resp, what).await
    }

    pub fn http(&self) -> &Client {
        &self.http
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > 300 {
        format!("{}…", trimmed.chars().take(300).collect::<String>())
    } else {
        trimmed.to_string()
    }
}

/// Map a non-success status to the right `DriveError`.
pub async fn check_status(resp: Response, what: &str) -> Result<Response, DriveError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(match status.as_u16() {
        429 | 503 => DriveError::RateLimited,
        401 | 421 | 450 => DriveError::SignInRequired(format!("{what}: session rejected")),
        s => DriveError::Api {
            status: s,
            message: format!("{what}: {}", snippet(&body)),
        },
    })
}

async fn expect_ok(resp: Response, what: &str) -> Result<(), DriveError> {
    check_status(resp, what).await.map(|_| ())
}

async fn decode_json<T: DeserializeOwned>(resp: Response, what: &str) -> Result<T, DriveError> {
    let resp = check_status(resp, what).await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(net)?;
    serde_json::from_str(&body).map_err(|e| DriveError::Api {
        status,
        message: format!("{what}: unexpected response ({e}): {}", snippet(&body)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_is_redacted() {
        let c = Credentials {
            apple_id: "me@example.com".into(),
            password: "s3cret".into(),
        };
        let s = format!("{c:?}");
        assert!(s.contains("me@example.com") && !s.contains("s3cret"));
    }

    #[test]
    fn snippet_truncates_long_bodies() {
        assert_eq!(snippet("  ok  "), "ok");
        let long = "x".repeat(400);
        let s = snippet(&long);
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), 301);
    }

    #[test]
    fn auth_url_paths_are_composed_as_apple_expects() {
        assert_eq!(
            format!("{AUTH_URL}/authorize/signin"),
            "https://idmsa.apple.com/appleauth/auth/authorize/signin"
        );
        assert_eq!(
            format!("{AUTH_URL}/2sv/trust"),
            "https://idmsa.apple.com/appleauth/auth/2sv/trust"
        );
    }
}
