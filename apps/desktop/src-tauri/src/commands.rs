//! One `#[tauri::command]` per row of docs/IPC.md section 4, with exactly those names and
//! argument names (`rename_all = "snake_case"` keeps the JS-side keys as written in the doc).
//! Every error is serialized as `IpcError`.

use crate::{AppState, Subscription};
use analyst_core::service::{agent_runner, SessionRuntime};
use analyst_protocol::evals::Mode;
use analyst_protocol::kernel::{
    Diagnostic, EqCheckResult, Equation, OutListResult, OutReadResult, PackageInfo,
};
use analyst_protocol::transcript::Event as TranscriptEvent;
use analyst_protocol::ui::*;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

type R<T> = Result<T, IpcError>;

fn rt_of(state: &State<'_, AppState>, id: &str) -> R<Arc<SessionRuntime>> {
    state.workbench.session(id).map_err(IpcError::from)
}

fn parse_mode(mode: &str) -> R<Mode> {
    mode.parse::<Mode>().map_err(IpcError::invalid)
}

// ---- app and sessions ----

#[tauri::command(rename_all = "snake_case")]
pub async fn app_info(state: State<'_, AppState>) -> R<AppInfo> {
    Ok(state.workbench.app_info())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sessions_list(state: State<'_, AppState>) -> R<Vec<SessionSummary>> {
    state.workbench.sessions_list().map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_create(
    state: State<'_, AppState>,
    repo_abs: String,
    target: Option<String>,
    model: Option<String>,
    init: Option<bool>,
) -> R<SessionInfo> {
    state
        .workbench
        .create_session(
            &repo_abs,
            target.as_deref(),
            model.as_deref(),
            init.unwrap_or(false),
        )
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_open(state: State<'_, AppState>, session: String) -> R<SessionInfo> {
    state
        .workbench
        .open_session(&session)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_close(state: State<'_, AppState>, session: String) -> R<()> {
    // Subscriptions of a closed session end too.
    if let Ok(mut subs) = state.subscriptions.lock() {
        let ids: Vec<String> = subs
            .iter()
            .filter(|(_, s)| s.session == session)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(s) = subs.remove(&id) {
                s.handle.abort();
            }
        }
    }
    state
        .workbench
        .close_session(&session)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_info(state: State<'_, AppState>, session: String) -> R<SessionInfo> {
    state
        .workbench
        .session_info(&session)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_rename(
    state: State<'_, AppState>,
    session: String,
    name: String,
) -> R<SessionSummary> {
    state
        .workbench
        .rename_session(&session, &name)
        .map_err(Into::into)
}

/// Opens a per-window channel; events flow until `session_unsubscribe` or the window closes.
#[tauri::command(rename_all = "snake_case")]
pub async fn session_subscribe(
    state: State<'_, AppState>,
    window: tauri::Window,
    session: String,
    channel: Channel<SessionEvent>,
) -> R<String> {
    let rt = rt_of(&state, &session)?;
    let id = format!("sub-{}", analyst_core::util::stamped_id());
    let mut rx = rt.subscribe();
    let sub_id = id.clone();
    let handle = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if channel.send(event).is_err() {
                        tracing::debug!("subscription {sub_id}: channel closed");
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("subscription {sub_id}: dropped {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    if let Ok(mut subs) = state.subscriptions.lock() {
        subs.insert(
            id.clone(),
            Subscription {
                window: window.label().to_string(),
                session,
                handle,
            },
        );
    }
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_unsubscribe(state: State<'_, AppState>, subscription: String) -> R<()> {
    let sub = state
        .subscriptions
        .lock()
        .ok()
        .and_then(|mut s| s.remove(&subscription));
    match sub {
        Some(s) => {
            s.handle.abort();
            Ok(())
        }
        None => Err(IpcError::not_found(format!(
            "subscription `{subscription}` not found"
        ))),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_set_target(
    state: State<'_, AppState>,
    session: String,
    target: String,
) -> R<TaskId> {
    rt_of(&state, &session)?
        .set_target_task(&target)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn session_set_policy(
    state: State<'_, AppState>,
    session: String,
    policy: PolicyPatch,
) -> R<PolicyView> {
    let rt = rt_of(&state, &session)?;
    let view = rt.set_policy(&policy)?;
    rt.emit_session_changed().await;
    Ok(view)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn layout_get(state: State<'_, AppState>, session: String) -> R<Option<String>> {
    state.workbench.layout_get(&session).map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn layout_save(state: State<'_, AppState>, session: String, layout: String) -> R<()> {
    state
        .workbench
        .layout_save(&session, &layout)
        .map_err(Into::into)
}

// ---- filesystem ----

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_tree(
    state: State<'_, AppState>,
    session: String,
    path: Option<String>,
    depth: Option<u32>,
) -> R<Vec<FileEntry>> {
    rt_of(&state, &session)?
        .fs_tree(path.as_deref(), depth)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_read(state: State<'_, AppState>, session: String, path: String) -> R<FileContent> {
    rt_of(&state, &session)?.fs_read(&path).map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_read_binary(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> R<FigureData> {
    rt_of(&state, &session)?
        .fs_read_binary(&path)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_write(
    state: State<'_, AppState>,
    session: String,
    path: String,
    content: String,
    expected_hash: Option<String>,
) -> R<WriteResult> {
    rt_of(&state, &session)?
        .fs_write(&path, &content, expected_hash.as_deref())
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_create(
    state: State<'_, AppState>,
    session: String,
    path: String,
    is_dir: bool,
) -> R<()> {
    rt_of(&state, &session)?
        .fs_create(&path, is_dir)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_delete(state: State<'_, AppState>, session: String, path: String) -> R<()> {
    rt_of(&state, &session)?
        .fs_delete(&path)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_rename(
    state: State<'_, AppState>,
    session: String,
    from: String,
    to: String,
) -> R<()> {
    rt_of(&state, &session)?
        .fs_rename(&from, &to)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fs_search(
    state: State<'_, AppState>,
    session: String,
    pattern: String,
    glob: Option<String>,
    max_results: Option<u32>,
) -> R<Vec<SearchHit>> {
    rt_of(&state, &session)?
        .fs_search(&pattern, glob.as_deref(), max_results)
        .await
        .map_err(Into::into)
}

// ---- index, checks, runs ----

#[tauri::command(rename_all = "snake_case")]
pub async fn index_get(state: State<'_, AppState>, session: String) -> R<IndexResult> {
    rt_of(&state, &session)?
        .index_get()
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn check_run(
    state: State<'_, AppState>,
    session: String,
    paths: Option<Vec<String>>,
) -> R<Vec<Diagnostic>> {
    rt_of(&state, &session)?
        .check_run(paths)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn run_start(
    state: State<'_, AppState>,
    session: String,
    script: String,
    args: Vec<String>,
    network: Option<bool>,
) -> R<RunView> {
    rt_of(&state, &session)?
        .run_start(&script, args, network)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn run_cancel(state: State<'_, AppState>, session: String, run_id: String) -> R<()> {
    rt_of(&state, &session)?
        .run_cancel(&run_id)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn runs_list(
    state: State<'_, AppState>,
    session: String,
    limit: Option<u32>,
) -> R<Vec<RunView>> {
    Ok(rt_of(&state, &session)?.runs_list(limit))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn run_get(state: State<'_, AppState>, session: String, run_id: String) -> R<RunDetail> {
    rt_of(&state, &session)?
        .run_get(&run_id)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn out_list(
    state: State<'_, AppState>,
    session: String,
    path: Option<String>,
    run_id: Option<String>,
) -> R<OutListResult> {
    rt_of(&state, &session)?
        .out_list(path, run_id)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn out_read(
    state: State<'_, AppState>,
    session: String,
    path: String,
    max_bytes: Option<u64>,
) -> R<OutReadResult> {
    rt_of(&state, &session)?
        .out_read(path, max_bytes)
        .await
        .map_err(Into::into)
}

// ---- figures and equations ----

#[tauri::command(rename_all = "snake_case")]
pub async fn figures_list(
    state: State<'_, AppState>,
    session: String,
    limit: Option<u32>,
) -> R<Vec<FigureGroup>> {
    Ok(rt_of(&state, &session)?.figures_list(limit))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn figure_data(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> R<FigureData> {
    rt_of(&state, &session)?
        .figure_data(&path)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn eq_extract(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> R<Vec<Equation>> {
    rt_of(&state, &session)?
        .eq_extract(&path)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn eq_check(
    state: State<'_, AppState>,
    session: String,
    path: String,
    function: String,
) -> R<EqCheckResult> {
    rt_of(&state, &session)?
        .eq_check(&path, &function)
        .await
        .map_err(Into::into)
}

// ---- environment ----

#[tauri::command(rename_all = "snake_case")]
pub async fn env_packages(state: State<'_, AppState>, session: String) -> R<Vec<PackageInfo>> {
    rt_of(&state, &session)?
        .env_packages()
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn env_add(
    state: State<'_, AppState>,
    session: String,
    packages: Vec<String>,
) -> R<TaskId> {
    rt_of(&state, &session)?
        .env_task("add", packages)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn env_remove(
    state: State<'_, AppState>,
    session: String,
    packages: Vec<String>,
) -> R<TaskId> {
    rt_of(&state, &session)?
        .env_task("remove", packages)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn env_sync(state: State<'_, AppState>, session: String) -> R<TaskId> {
    rt_of(&state, &session)?
        .env_task("sync", vec![])
        .map_err(Into::into)
}

// ---- conversations and chat ----

#[tauri::command(rename_all = "snake_case")]
pub async fn conversation_new(
    state: State<'_, AppState>,
    session: String,
    title: Option<String>,
) -> R<ConversationSummary> {
    let rt = rt_of(&state, &session)?;
    let c = rt.conversation_new(title.as_deref())?;
    rt.emit_session_changed().await;
    Ok(c)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversation_select(
    state: State<'_, AppState>,
    session: String,
    conversation: String,
) -> R<()> {
    let rt = rt_of(&state, &session)?;
    rt.conversation_select(&conversation)?;
    rt.emit_session_changed().await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversation_transcript(
    state: State<'_, AppState>,
    session: String,
    conversation: String,
) -> R<Vec<TranscriptEvent>> {
    rt_of(&state, &session)?
        .conversation_transcript(&conversation)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_send(
    state: State<'_, AppState>,
    session: String,
    conversation: String,
    text: String,
    attachments: Option<Vec<Attachment>>,
) -> R<TurnId> {
    let rt = rt_of(&state, &session)?;
    let conv = rt.conversation_runtime(&conversation)?;
    agent_runner::start_turn(rt, conv, &text, attachments.unwrap_or_default())
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_cancel(
    state: State<'_, AppState>,
    session: String,
    conversation: String,
) -> R<()> {
    let rt = rt_of(&state, &session)?;
    let conv = rt.conversation_runtime(&conversation)?;
    agent_runner::cancel_turn(&rt, &conv).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_set_mode(
    state: State<'_, AppState>,
    session: String,
    conversation: String,
    mode: String,
) -> R<()> {
    let mode = parse_mode(&mode)?;
    rt_of(&state, &session)?
        .chat_set_mode(&conversation, mode)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_set_model(
    state: State<'_, AppState>,
    session: String,
    conversation: String,
    model: String,
) -> R<()> {
    rt_of(&state, &session)?
        .chat_set_model(&conversation, &model)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn proposal_respond(
    state: State<'_, AppState>,
    session: String,
    proposal_id: String,
    decision: ProposalDecision,
) -> R<()> {
    rt_of(&state, &session)?
        .proposal_respond(&proposal_id, decision)
        .map_err(Into::into)
}

// ---- models and targets ----

#[tauri::command(rename_all = "snake_case")]
pub async fn models_list(state: State<'_, AppState>) -> R<Vec<ModelEntry>> {
    let _ = state.workbench.reload_global();
    Ok(state.workbench.models_list())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn model_probe(state: State<'_, AppState>, model: String) -> R<ModelEntry> {
    state
        .workbench
        .model_probe(&model)
        .await
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn targets_list(state: State<'_, AppState>) -> R<Vec<TargetInfo>> {
    let _ = state.workbench.reload_global();
    Ok(state.workbench.targets_list())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn target_bootstrap(state: State<'_, AppState>, target: String) -> R<TaskId> {
    state
        .workbench
        .target_bootstrap(&target)
        .map_err(Into::into)
}

// ---- terminals ----

#[tauri::command(rename_all = "snake_case")]
pub async fn terminal_open(
    state: State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> R<TerminalId> {
    state
        .workbench
        .terminal_open(&session, cols, rows)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn terminal_write(
    state: State<'_, AppState>,
    terminal_id: String,
    data: String,
) -> R<()> {
    state
        .workbench
        .terminal_write(&terminal_id, &data)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn terminal_resize(
    state: State<'_, AppState>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> R<()> {
    state
        .workbench
        .terminal_resize(&terminal_id, cols, rows)
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn terminal_close(state: State<'_, AppState>, terminal_id: String) -> R<()> {
    state
        .workbench
        .terminal_close(&terminal_id)
        .map_err(Into::into)
}

// ---- dialogs ----

/// Native folder picker; `null` when cancelled.
#[tauri::command(rename_all = "snake_case")]
pub async fn dialog_pick_directory(
    app: tauri::AppHandle,
    title: Option<String>,
) -> R<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    if let Some(t) = title {
        dialog = dialog.set_title(t);
    }
    dialog.pick_folder(move |p| {
        let _ = tx.send(p);
    });
    let picked = rx
        .await
        .map_err(|_| IpcError::internal("the folder dialog was dropped"))?;
    match picked {
        None => Ok(None),
        Some(p) => {
            let path = p
                .into_path()
                .map_err(|e| IpcError::invalid(format!("not a filesystem path: {e}")))?;
            Ok(Some(path.to_string_lossy().to_string()))
        }
    }
}
