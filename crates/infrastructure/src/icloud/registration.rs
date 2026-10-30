//! Retry only explicit busy replies for the same already-uploaded document.
use super::{
    auth::{decode_json, net, status_error, IcloudClient},
    wire::{DocumentUpdateResponse, ServiceError, UpdateDocument},
};
use reqwest::header::{HeaderValue, RETRY_AFTER};
use std::time::Duration;
use wattdrive_domain::DriveError;

pub(super) async fn register(
    client: &IcloudClient,
    url: &str,
    update: &UpdateDocument<'_>,
) -> Result<DocumentUpdateResponse, DriveError> {
    // The caller retains the account's write permit through these waits.
    let mut retries = 0;
    loop {
        let response = client
            .service_send(&|http, headers| http.post(url).headers(headers).json(update))
            .await?;
        let status = response.status();
        if status.is_success() {
            return decode_json(response, "register upload").await;
        }
        let retry_after = response.headers().get(RETRY_AFTER).cloned();
        let body = response.text().await.map_err(net)?;
        if matches!(status.as_u16(), 409 | 502) && retries < 3 {
            if let Ok(error) = serde_json::from_str::<ServiceError>(&body) {
                if let Some(code) = error.busy_code() {
                    if let Some(wait) = retry_wait(retries, &error, retry_after.as_ref()) {
                        retries += 1;
                        tracing::info!(
                            retries,
                            code,
                            wait_secs = wait.as_secs(),
                            "iCloud registration busy; retrying"
                        );
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                }
            }
        }
        // Unknown responses and transport failures are never blindly replayed.
        return Err(status_error(status.as_u16(), &body, "register upload"));
    }
}

fn retry_wait(
    attempt: u32,
    error: &ServiceError,
    header: Option<&HeaderValue>,
) -> Option<Duration> {
    // Unparseable or long server delays stop the pass instead of ignoring them.
    let header_secs = match header {
        Some(value) => value.to_str().ok()?.parse::<u64>().ok()?,
        None => 0,
    };
    let secs = header_secs
        .max(error.retry_after.unwrap_or(0))
        .max(1 << attempt);
    (secs <= 30).then(|| Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_wait_respects_server_delay_and_is_bounded() {
        let error = serde_json::from_str(r#"{"retryAfter":10}"#).unwrap();
        assert_eq!(retry_wait(0, &error, None), Some(Duration::from_secs(10)));
        assert_eq!(
            retry_wait(0, &error, Some(&HeaderValue::from_static("15"))),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            retry_wait(0, &error, Some(&HeaderValue::from_static("60"))),
            None
        );
        assert_eq!(
            retry_wait(0, &error, Some(&HeaderValue::from_static("invalid"))),
            None
        );
        let error = serde_json::from_str("{}").unwrap();
        assert_eq!(retry_wait(2, &error, None), Some(Duration::from_secs(4)));
    }
}
