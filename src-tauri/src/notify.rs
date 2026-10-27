//! Desktop notifications, off the Tokio workers: notify-rust's `zbus::block_on`
//! aborts the process when called from inside a runtime.

pub fn notify(summary: &str, body: &str) {
    let (summary, body) = (summary.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        #[cfg(windows)]
        notification.app_id("co.swatto.wattdrive");
        let result = notification
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
