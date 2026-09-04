//! Main-window show / hide / toggle used by the tray and close-to-tray path.

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Manager};

use crate::USER_HID_WINDOW;

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Minimized counts as not shown so a tray click restores rather than hides.
fn tray_click_hides(visible: bool, minimized: bool) -> bool {
    visible && !minimized
}

pub(crate) fn toggle_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    if tray_click_hides(visible, minimized) {
        USER_HID_WINDOW.store(true, Ordering::SeqCst);
        let _ = window.hide();
    } else {
        show_main(app);
    }
}

#[cfg(test)]
mod tests {
    use super::tray_click_hides;

    #[test]
    fn tray_click_hides_only_a_shown_unminimized_window() {
        assert!(tray_click_hides(true, false));
        assert!(!tray_click_hides(false, false));
        assert!(!tray_click_hides(true, true));
        assert!(!tray_click_hides(false, true));
    }
}
