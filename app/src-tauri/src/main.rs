// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's accelerated compositing costs each web process ~150-200 MB more
    // when GL is software-rendered (VMs, machines without a usable GPU driver),
    // and this UI does not need it. Measured in the Lima VM: total PSS of the app
    // and its WebKit processes 684 MB with it, 294 MB without, same rendering.
    // Set WEBKIT_DISABLE_COMPOSITING_MODE=0 to get it back.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        // SAFETY: first statement of main, before any other thread exists.
        unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1") };
    }
    intent_app_lib::run()
}
