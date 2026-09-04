fn main() {
    // NVIDIA + Hyprland WebKit DMA-BUF protocol error 71 — apply before
    // anything that can load GTK/WebKit (including lib::run → Builder).
    #[cfg(target_os = "linux")]
    wattdrive_desktop_lib::linux_webkit::apply_session_quirks();

    wattdrive_desktop_lib::run();
}
