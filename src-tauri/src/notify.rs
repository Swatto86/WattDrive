//! Desktop notifications, off the Tokio workers: notify-rust's `zbus::block_on`
//! aborts the process when called from inside a runtime.

pub fn notify(summary: &str, body: &str) {
    let (summary, body) = (summary.to_string(), body.to_string());
    std::thread::spawn(move || {
        let result = notify_rust::Notification::new()
            .appname("WattDrive")
            .summary(&summary)
            .body(&body)
            .icon("co.swatto.wattdrive")
            .show();
        if let Err(e) = result {
            tracing::warn!("desktop notification failed: {e}");
        }
    });
}
