//! The state Apple hands back across a sign-in — cookies, session/trust tokens,
//! the `scnt` counter — and the header sets each endpoint family expects.
//! Persisted as [`SavedSession`] so the app can pick up where it left off.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use serde::{Deserialize, Serialize};

use super::wire::AccountInfo;

pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3.1 Safari/605.1.15";
pub const BASE_URL: &str = "https://www.icloud.com";
pub const SETUP_URL: &str = "https://setup.icloud.com/setup/ws/1";
pub const AUTH_URL: &str = "https://idmsa.apple.com/appleauth/auth";
/// The public "widget key" icloud.com's own sign-in frame identifies with.
pub const DEFAULT_CLIENT_ID: &str =
    "d39ba9916b7251055b22c7f910e2ea796ee65e98b2ddecea8f5dde8d9d1a815d";
/// Cookie iCloud issues once an Advanced Data Protection account has approved
/// web access to Drive; without it the drive endpoints answer 423.
pub const PCS_DOCUMENTS_COOKIE: &str = "X-APPLE-WEBAUTH-PCS-Documents";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
}

/// Everything worth keeping between runs. Held in the OS keyring as JSON.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedSession {
    pub session_token: String,
    pub scnt: String,
    pub session_id: String,
    pub account_country: String,
    pub trust_token: String,
    pub client_id: String,
    pub auth_attributes: String,
    pub cookies: Vec<Cookie>,
    pub account_info: AccountInfo,
}

impl SavedSession {
    /// A session that can talk to the drive endpoints without signing in again
    /// (it may still be rejected and trigger a silent re-auth).
    pub fn looks_usable(&self) -> bool {
        !self.cookies.is_empty() && !self.account_info.webservices.is_empty()
    }
}

pub struct Session {
    pub saved: SavedSession,
    pub frame_id: String,
    pub needs_2fa: bool,
}

impl Session {
    pub fn new(saved: Option<SavedSession>) -> Self {
        let mut saved = saved.unwrap_or_default();
        if saved.client_id.is_empty() {
            saved.client_id = DEFAULT_CLIENT_ID.to_string();
        }
        Self {
            saved,
            frame_id: uuid::Uuid::new_v4().to_string().to_lowercase(),
            needs_2fa: false,
        }
    }

    /// Pull Apple's session headers and cookies out of a response.
    pub fn absorb(&mut self, headers: &HeaderMap) {
        for raw in headers.get_all(SET_COOKIE) {
            if let Some((name, value)) = parse_set_cookie(raw.to_str().unwrap_or("")) {
                self.merge_cookie(name, value);
            }
        }
        let take = |name: &str, slot: &mut String| {
            if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
                if !v.is_empty() {
                    *slot = v.to_string();
                }
            }
        };
        take(
            "X-Apple-ID-Account-Country",
            &mut self.saved.account_country,
        );
        take("X-Apple-ID-Session-Id", &mut self.saved.session_id);
        take("X-Apple-Session-Token", &mut self.saved.session_token);
        take("X-Apple-TwoSV-Trust-Token", &mut self.saved.trust_token);
        take("scnt", &mut self.saved.scnt);
        take("X-Apple-Auth-Attributes", &mut self.saved.auth_attributes);
    }

    fn merge_cookie(&mut self, name: &str, value: &str) {
        let pos = self.saved.cookies.iter().position(|c| c.name == name);
        match (pos, value.is_empty()) {
            (Some(i), true) => {
                self.saved.cookies.remove(i);
            }
            (None, true) => {}
            (Some(i), false) => self.saved.cookies[i].value = value.to_string(),
            (None, false) => self.saved.cookies.push(Cookie {
                name: name.to_string(),
                value: value.to_string(),
            }),
        }
    }

    pub fn has_cookie(&self, name: &str) -> bool {
        self.saved.cookies.iter().any(|c| c.name == name)
    }

    pub fn cookie_header(&self) -> String {
        self.saved
            .cookies
            .iter()
            .filter(|c| !c.value.is_empty())
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn requires_2fa(&self) -> bool {
        if self.needs_2fa {
            return true;
        }
        let info = &self.saved.account_info;
        info.ds_info.as_ref().is_some_and(|d| d.hsa_version == 2) && info.hsa_challenge_required
    }

    pub fn webservice_url(&self, key: &str) -> Option<&str> {
        self.saved
            .account_info
            .webservices
            .get(key)
            .map(|w| w.url.as_str())
            .filter(|u| !u.is_empty())
    }

    /// Headers for idmsa.apple.com (SRP, 2FA, trust).
    pub fn auth_headers(&self) -> HeaderMap {
        let frame = format!("auth-{}", self.frame_id);
        let origin = AUTH_URL.trim_end_matches("/appleauth/auth");
        let mut h = HeaderMap::new();
        put(&mut h, "Accept", "application/json");
        put(&mut h, "Content-Type", "application/json");
        put(&mut h, "User-Agent", USER_AGENT);
        put(&mut h, "Origin", origin);
        put(&mut h, "Referer", &format!("{origin}/"));
        put(&mut h, "X-Apple-Widget-Key", &self.saved.client_id);
        put(&mut h, "X-Apple-OAuth-Client-Id", &self.saved.client_id);
        put(&mut h, "X-Apple-OAuth-Client-Type", "firstPartyAuth");
        put(&mut h, "X-Apple-OAuth-Redirect-URI", BASE_URL);
        put(&mut h, "X-Apple-OAuth-Require-Grant-Code", "true");
        put(&mut h, "X-Apple-OAuth-Response-Mode", "web_message");
        put(&mut h, "X-Apple-OAuth-Response-Type", "code");
        put(&mut h, "X-Apple-OAuth-State", &frame);
        put(&mut h, "X-Apple-Frame-Id", &frame);
        put(&mut h, "X-Requested-With", "XMLHttpRequest");
        put(&mut h, "X-Apple-Mandate-Security-Upgrade", "0");
        put(&mut h, "X-Apple-I-Require-UE", "true");
        put(
            &mut h,
            "X-Apple-I-FD-Client-Info",
            &format!(r#"{{"U":"{USER_AGENT}","L":"en-US","Z":"GMT+00:00","V":"1.1","F":""}}"#),
        );
        if !self.saved.auth_attributes.is_empty() {
            put(
                &mut h,
                "X-Apple-Auth-Attributes",
                &self.saved.auth_attributes,
            );
        }
        if !self.saved.scnt.is_empty() {
            put(&mut h, "scnt", &self.saved.scnt);
        }
        if !self.saved.session_id.is_empty() {
            put(&mut h, "X-Apple-ID-Session-Id", &self.saved.session_id);
        }
        h
    }

    /// Headers for setup.icloud.com and the per-account drive/docs hosts.
    pub fn service_headers(&self) -> HeaderMap {
        let mut h = common_headers();
        put(&mut h, "Cookie", &self.cookie_header());
        h
    }
}

/// Headers every icloud.com request carries, with no cookies.
pub fn common_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    put(&mut h, "Content-Type", "application/json");
    put(&mut h, "Origin", BASE_URL);
    put(&mut h, "Referer", &format!("{BASE_URL}/"));
    put(&mut h, "User-Agent", USER_AGENT);
    h
}

fn put(h: &mut HeaderMap, name: &str, value: &str) {
    if let (Ok(n), Ok(v)) = (
        HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(value),
    ) {
        h.insert(n, v);
    }
}

/// `name=value; Path=/; ...` → (name, value). A value of `""` counts as empty.
fn parse_set_cookie(raw: &str) -> Option<(&str, &str)> {
    let first = raw.split(';').next()?.trim();
    let (name, value) = first.split_once('=')?;
    let value = value.trim();
    let value = if value == "\"\"" { "" } else { value };
    Some((name.trim(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (n, v) in pairs {
            h.append(
                HeaderName::from_bytes(n.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn absorb_merges_cookies_and_session_headers() {
        let mut s = Session::new(None);
        s.absorb(&headers(&[
            (
                "set-cookie",
                "X-APPLE-WEBAUTH-USER=\"v1\"; Domain=.icloud.com; Path=/; Secure",
            ),
            ("set-cookie", "X-APPLE-WEBAUTH-TOKEN=t1; Path=/"),
            ("scnt", "AAAA"),
            ("X-Apple-Session-Token", "st"),
            ("X-Apple-TwoSV-Trust-Token", "tt"),
            ("X-Apple-ID-Session-Id", "sid"),
            ("X-Apple-ID-Account-Country", "GBR"),
        ]));
        assert_eq!(
            s.cookie_header(),
            "X-APPLE-WEBAUTH-USER=\"v1\"; X-APPLE-WEBAUTH-TOKEN=t1"
        );
        assert_eq!(s.saved.scnt, "AAAA");
        assert_eq!(s.saved.session_token, "st");
        assert_eq!(s.saved.trust_token, "tt");
        assert_eq!(s.saved.session_id, "sid");
        assert_eq!(s.saved.account_country, "GBR");

        // replace one, delete one via empty value, ignore an unknown deletion
        s.absorb(&headers(&[
            ("set-cookie", "X-APPLE-WEBAUTH-TOKEN=t2; Path=/"),
            ("set-cookie", "X-APPLE-WEBAUTH-USER=\"\"; Max-Age=0"),
            ("set-cookie", "X-NEVER-SEEN=; Max-Age=0"),
        ]));
        assert_eq!(s.cookie_header(), "X-APPLE-WEBAUTH-TOKEN=t2");
        assert!(!s.has_cookie("X-NEVER-SEEN"));
    }

    #[test]
    fn auth_headers_carry_the_frame_and_session_state_when_present() {
        let mut s = Session::new(None);
        let h = s.auth_headers();
        assert_eq!(h["X-Apple-Widget-Key"], DEFAULT_CLIENT_ID);
        assert_eq!(h["X-Apple-Frame-Id"], format!("auth-{}", s.frame_id));
        assert!(
            h.get("scnt").is_none(),
            "no scnt before the server issues one"
        );
        assert_eq!(h["Origin"], "https://idmsa.apple.com");
        s.saved.scnt = "S".into();
        s.saved.session_id = "I".into();
        let h = s.auth_headers();
        assert_eq!(h["scnt"], "S");
        assert_eq!(h["X-Apple-ID-Session-Id"], "I");
    }

    #[test]
    fn service_headers_include_cookies_and_icloud_origin() {
        let mut s = Session::new(None);
        s.absorb(&headers(&[("set-cookie", "a=1"), ("set-cookie", "b=2")]));
        let h = s.service_headers();
        assert_eq!(h["Cookie"], "a=1; b=2");
        assert_eq!(h["Origin"], BASE_URL);
        assert_eq!(h["Referer"], "https://www.icloud.com/");
    }

    #[test]
    fn saved_session_usability_and_2fa_detection() {
        let mut s = Session::new(None);
        assert!(!s.saved.looks_usable());
        assert!(!s.requires_2fa());
        s.needs_2fa = true;
        assert!(s.requires_2fa());
        let saved: SavedSession = serde_json::from_str(
            r#"{"session_token":"","scnt":"","session_id":"","account_country":"","trust_token":"",
                "client_id":"","auth_attributes":"","cookies":[{"name":"a","value":"1"}],
                "account_info":{"dsInfo":{"hsaVersion":2},"hsaChallengeRequired":true,
                "webservices":{"drivews":{"url":"https://d"}}}}"#,
        )
        .unwrap();
        assert!(saved.looks_usable());
        let s = Session::new(Some(saved));
        assert!(s.requires_2fa(), "hsa2 + challenge flag means 2FA pending");
        assert_eq!(s.webservice_url("drivews"), Some("https://d"));
        assert_eq!(s.webservice_url("docws"), None);
        assert_eq!(
            s.saved.client_id, DEFAULT_CLIENT_ID,
            "empty id is defaulted"
        );
    }
}
