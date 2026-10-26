//! Personal Analyst desktop backend (Tauri 2). The commands in [`commands`] are thin wrappers over
//! `analyst_core::service::Workbench`; session events reach the renderer through
//! `tauri::ipc::Channel<SessionEvent>` subscriptions opened with `session_subscribe`.

pub mod commands;

use analyst_core::service::{Workbench, WorkbenchOptions};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// One `session_subscribe` channel: the forwarding task and the window that opened it.
pub struct Subscription {
    pub window: String,
    pub session: String,
    pub handle: tokio::task::JoinHandle<()>,
}

/// Managed Tauri state.
pub struct AppState {
    pub workbench: Arc<Workbench>,
    pub subscriptions: Mutex<HashMap<String, Subscription>>,
}

impl AppState {
    pub fn new(workbench: Arc<Workbench>) -> Self {
        AppState {
            workbench,
            subscriptions: Mutex::new(HashMap::new()),
        }
    }

    /// Drop every subscription a window opened (window closed).
    pub fn drop_window_subscriptions(&self, window: &str) {
        if let Ok(mut subs) = self.subscriptions.lock() {
            let ids: Vec<String> = subs
                .iter()
                .filter(|(_, s)| s.window == window)
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                if let Some(s) = subs.remove(&id) {
                    s.handle.abort();
                }
            }
        }
    }
}

/// Build the workbench from `$ANALYST_HOME` (default `~/.analyst`) and the environment:
/// `ANALYST_SANDBOX` overrides the kernel sandbox kind.
pub fn workbench_from_env() -> anyhow::Result<Arc<Workbench>> {
    let home = analyst_core::config::analyst_home();
    let mut opts = WorkbenchOptions::new(home);
    if let Ok(s) = std::env::var("ANALYST_SANDBOX") {
        if !s.is_empty() {
            opts.sandbox = Some(s);
        }
    }
    let wb = Workbench::new(opts).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Arc::new(wb))
}

/// The command names registered in `generate_handler!`, kept in one place so a test can check
/// them against docs/IPC.md section 4.
pub const COMMANDS: &[&str] = &[
    "app_info",
    "sessions_list",
    "session_create",
    "session_open",
    "session_close",
    "session_info",
    "session_rename",
    "session_subscribe",
    "session_unsubscribe",
    "session_set_target",
    "session_set_policy",
    "layout_get",
    "layout_save",
    "fs_tree",
    "fs_read",
    "fs_read_binary",
    "fs_write",
    "fs_create",
    "fs_delete",
    "fs_rename",
    "fs_search",
    "index_get",
    "check_run",
    "run_start",
    "run_cancel",
    "runs_list",
    "run_get",
    "out_list",
    "out_read",
    "figures_list",
    "figure_data",
    "eq_extract",
    "eq_check",
    "env_packages",
    "env_add",
    "env_remove",
    "env_sync",
    "conversation_new",
    "conversation_select",
    "conversation_transcript",
    "chat_send",
    "chat_cancel",
    "chat_set_mode",
    "chat_set_model",
    "proposal_respond",
    "models_list",
    "model_probe",
    "targets_list",
    "target_bootstrap",
    "terminal_open",
    "terminal_write",
    "terminal_resize",
    "terminal_close",
    "dialog_pick_directory",
];

/// Configure and run the app.
pub fn run(workbench: Arc<Workbench>) -> anyhow::Result<()> {
    use commands::*;
    let state = AppState::new(workbench.clone());
    let shutdown_wb = workbench.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let label = window.label().to_string();
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    state.drop_window_subscriptions(&label);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            sessions_list,
            session_create,
            session_open,
            session_close,
            session_info,
            session_rename,
            session_subscribe,
            session_unsubscribe,
            session_set_target,
            session_set_policy,
            layout_get,
            layout_save,
            fs_tree,
            fs_read,
            fs_read_binary,
            fs_write,
            fs_create,
            fs_delete,
            fs_rename,
            fs_search,
            index_get,
            check_run,
            run_start,
            run_cancel,
            runs_list,
            run_get,
            out_list,
            out_read,
            figures_list,
            figure_data,
            eq_extract,
            eq_check,
            env_packages,
            env_add,
            env_remove,
            env_sync,
            conversation_new,
            conversation_select,
            conversation_transcript,
            chat_send,
            chat_cancel,
            chat_set_mode,
            chat_set_model,
            proposal_respond,
            models_list,
            model_probe,
            targets_list,
            target_bootstrap,
            terminal_open,
            terminal_write,
            terminal_resize,
            terminal_close,
            dialog_pick_directory,
        ])
        .build(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("building the Tauri app: {e}"))?
        .run(move |_app, event| {
            if let tauri::RunEvent::Exit = event {
                let wb = shutdown_wb.clone();
                tauri::async_runtime::block_on(async move { wb.shutdown().await });
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::COMMANDS;

    /// Copied from docs/IPC.md section 4 (the `command` column).
    const IPC_MD_COMMANDS: &[&str] = &[
        "app_info",
        "sessions_list",
        "session_create",
        "session_open",
        "session_close",
        "session_info",
        "session_rename",
        "session_subscribe",
        "session_unsubscribe",
        "session_set_target",
        "session_set_policy",
        "layout_get",
        "layout_save",
        "fs_tree",
        "fs_read",
        "fs_read_binary",
        "fs_write",
        "fs_create",
        "fs_delete",
        "fs_rename",
        "fs_search",
        "index_get",
        "check_run",
        "run_start",
        "run_cancel",
        "runs_list",
        "run_get",
        "out_list",
        "out_read",
        "figures_list",
        "figure_data",
        "eq_extract",
        "eq_check",
        "env_packages",
        "env_add",
        "env_remove",
        "env_sync",
        "conversation_new",
        "conversation_select",
        "conversation_transcript",
        "chat_send",
        "chat_cancel",
        "chat_set_mode",
        "chat_set_model",
        "proposal_respond",
        "models_list",
        "model_probe",
        "targets_list",
        "target_bootstrap",
        "terminal_open",
        "terminal_write",
        "terminal_resize",
        "terminal_close",
        "dialog_pick_directory",
    ];

    #[test]
    fn every_ipc_md_command_is_registered() {
        for c in IPC_MD_COMMANDS {
            assert!(
                COMMANDS.contains(c),
                "command `{c}` from IPC.md is not registered"
            );
        }
        for c in COMMANDS {
            assert!(
                IPC_MD_COMMANDS.contains(c),
                "command `{c}` is not in IPC.md"
            );
        }
        assert_eq!(COMMANDS.len(), IPC_MD_COMMANDS.len());
    }

    /// The registered list must match the source of `generate_handler!` in this file.
    #[test]
    fn generate_handler_matches_command_list() {
        let src = include_str!("lib.rs");
        let start = src.find("generate_handler![").expect("handler list");
        let end = src[start..].find("])").expect("handler list end") + start;
        let body = &src[start + "generate_handler![".len()..end];
        let names: Vec<&str> = body
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(names, COMMANDS);
    }
}
