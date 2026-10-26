//! `analyst-desktop`: the Personal Analyst workbench.

// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // One tokio runtime for the core (kernels, watchers, agent turns) and for Tauri's async
    // commands, so `tokio::spawn` works everywhere in the service layer.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("analyst")
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            std::process::exit(2);
        }
    };
    tauri::async_runtime::set(runtime.handle().clone());
    let _guard = runtime.enter();

    let workbench = match analyst_desktop::workbench_from_env() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    };
    tracing::info!("analyst home: {}", workbench.home().display());
    if let Err(e) = analyst_desktop::run(workbench) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
