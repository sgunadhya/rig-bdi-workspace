fn main() {
    let demo_mode = std::env::args().any(|arg| arg == "--demo");
    #[cfg(feature = "tauri-app")]
    if !demo_mode {
        src_tauri::run_tauri();
        return;
    }

    let result = if demo_mode {
        src_tauri::run_demo()
    } else {
        src_tauri::run()
    };

    if let Err(err) = result {
        eprintln!("failed to start validation app backend: {err}");
    }
}
