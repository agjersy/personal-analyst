//! Service-level unit tests with the scripted model client (no kernel, no network).

use super::*;
use crate::providers::ScriptedModelClient;
use analyst_protocol::model::{ModelEvent, StopReason};
use analyst_protocol::transcript::Decision;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::broadcast;

struct Fixture {
    _home: tempfile::TempDir,
    _repo: tempfile::TempDir,
    wb: Arc<Workbench>,
    session: String,
    rt: Arc<SessionRuntime>,
}

async fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let mut opts = WorkbenchOptions::new(home.path().to_path_buf());
    opts.autostart_kernel = false;
    opts.init = crate::init::InitOptions {
        run_uv: false,
        git: true,
    };
    let wb = Arc::new(Workbench::new(opts).unwrap());
    let info = wb
        .create_session(&repo.path().to_string_lossy(), None, None, true)
        .await
        .unwrap();
    let rt = wb.session(&info.summary.id).unwrap();
    Fixture {
        _home: home,
        _repo: repo,
        wb,
        session: info.summary.id,
        rt,
    }
}

fn call(id: &str, name: &str, args: Value) -> ModelEvent {
    ModelEvent::ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args,
    }
}

fn stop_tools() -> ModelEvent {
    ModelEvent::Stop {
        reason: StopReason::ToolCalls,
        message: None,
    }
}

async fn next_matching<F>(rx: &mut broadcast::Receiver<SessionEvent>, pred: F) -> SessionEvent
where
    F: Fn(&SessionEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(e)) if pred(&e) => return e,
            Ok(Ok(_)) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            other => panic!("event not received: {other:?}"),
        }
    }
}

#[tokio::test]
async fn session_lifecycle_and_fs_ops() {
    let fx = fixture().await;
    let info = fx.wb.session_info(&fx.session).await.unwrap();
    assert!(info.summary.open);
    assert_eq!(
        info.target_kind,
        analyst_protocol::config::TargetKind::Local
    );
    assert_eq!(info.kernel, KernelState::Stopped);
    assert_eq!(info.conversations.len(), 1);
    assert_eq!(info.active_conversation, info.conversations[0].id);
    assert_eq!(
        info.policy.edits,
        analyst_protocol::config::EditPolicy::Review
    );

    let list = fx.wb.sessions_list().unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].open);
    let renamed = fx.wb.rename_session(&fx.session, "decay").unwrap();
    assert_eq!(renamed.name, "decay");

    // Tree: template dirs, .venv and __pycache__ hidden, out/ collapsed.
    std::fs::create_dir_all(fx.rt.repo.join(".venv/bin")).unwrap();
    std::fs::create_dir_all(fx.rt.repo.join("analysis/__pycache__")).unwrap();
    std::fs::write(fx.rt.repo.join("analysis/__pycache__/x.pyc"), "x").unwrap();
    let tree = fx.rt.fs_tree(None, Some(3)).unwrap();
    let names: Vec<&str> = tree.iter().map(|e| e.path.as_str()).collect();
    assert!(names.contains(&"scripts"));
    assert!(names.contains(&"analysis"));
    assert!(names.contains(&".analyst"));
    assert!(!names.contains(&".venv"));
    assert!(!names.contains(&".git"));
    let out = tree.iter().find(|e| e.path == "out").unwrap();
    assert!(out.children.is_none());
    let analysis = tree.iter().find(|e| e.path == "analysis").unwrap();
    assert!(analysis
        .children
        .as_ref()
        .unwrap()
        .iter()
        .all(|c| !c.path.contains("__pycache__")));

    // Write + conflict detection through the runtime.
    let c = fx.rt.fs_read("analysis/__init__.py").unwrap();
    let w = fx
        .rt
        .fs_write("analysis/__init__.py", "x = 1\n", Some(&c.hash))
        .unwrap();
    let err = fx
        .rt
        .fs_write("analysis/__init__.py", "y = 1\n", Some(&c.hash))
        .unwrap_err();
    assert_eq!(err.code, IpcErrorCode::Conflict);
    fx.rt
        .fs_write("analysis/__init__.py", "y = 1\n", Some(&w.hash))
        .unwrap();

    // Layout round trip and policy override.
    assert_eq!(fx.wb.layout_get(&fx.session).unwrap(), None);
    fx.wb.layout_save(&fx.session, "{\"a\":1}").unwrap();
    assert_eq!(
        fx.wb.layout_get(&fx.session).unwrap().as_deref(),
        Some("{\"a\":1}")
    );
    let p = fx
        .rt
        .set_policy(&PolicyPatch {
            edits: Some(analyst_protocol::config::EditPolicy::Auto),
            max_steps: Some(7),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(p.edits, analyst_protocol::config::EditPolicy::Auto);
    assert_eq!(p.max_steps, 7);
    assert_eq!(fx.rt.effective_policy().max_steps, 7);
    assert_eq!(
        crate::config::load_project(&fx.rt.repo)
            .unwrap()
            .policy
            .max_steps,
        40,
        "override is not written to project.toml"
    );

    // Conversations.
    let c2 = fx.rt.conversation_new(Some("second")).unwrap();
    assert_eq!(
        fx.rt.meta().active_conversation.as_deref(),
        Some(c2.id.as_str())
    );
    fx.rt
        .conversation_select(&info.active_conversation)
        .unwrap();
    assert_eq!(
        fx.rt.conversation_select("nope").unwrap_err().code,
        IpcErrorCode::NotFound
    );
    assert!(fx.rt.conversation_transcript(&c2.id).unwrap().is_empty());

    // Kernel-dependent commands: `kernel_offline` when no kernel can start (no tooling
    // interpreter), `not_found` from the kernel when one is available in this checkout.
    let err = fx.rt.run_cancel("x").await.unwrap_err();
    assert!(
        matches!(
            err.code,
            IpcErrorCode::KernelOffline | IpcErrorCode::NotFound
        ),
        "{err:?}"
    );

    fx.wb.close_session(&fx.session).await.unwrap();
    assert_eq!(
        fx.wb.session_info(&fx.session).await.unwrap_err().code,
        IpcErrorCode::NotFound
    );
    assert!(!fx.wb.sessions_list().unwrap()[0].open);
    // Reopen is idempotent.
    let again = fx.wb.open_session(&fx.session).await.unwrap();
    assert_eq!(again.summary.name, "decay");
    fx.wb.close_session(&fx.session).await.unwrap();
}

#[tokio::test]
async fn ui_approver_round_trip_accepts_with_modified_content() {
    let fx = fixture().await;
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            ModelEvent::Text {
                delta: "Editing.".into(),
            },
            call(
                "s",
                "declare_scope",
                json!({"files": ["analysis/__init__.py"]}),
            ),
            call(
                "e",
                "fs_propose_edit",
                json!({"path": "analysis/__init__.py", "content": "PROPOSED = 1\n"}),
            ),
            stop_tools(),
        ],
        vec![
            ModelEvent::Text {
                delta: "Done.".into(),
            },
            ModelEvent::Usage {
                input_tokens: 11,
                output_tokens: 3,
            },
        ],
    ]));
    fx.rt.set_model_override(Some(model.clone()));
    let conv_id = fx.rt.meta().active_conversation.unwrap();
    let conv = fx.rt.conversation_runtime(&conv_id).unwrap();
    let mut rx = fx.rt.subscribe();
    let turn = agent_runner::start_turn(fx.rt.clone(), conv.clone(), "edit the init", vec![])
        .await
        .unwrap();
    assert!(conv.busy());
    assert_eq!(
        agent_runner::start_turn(fx.rt.clone(), conv.clone(), "again", vec![])
            .await
            .unwrap_err()
            .code,
        IpcErrorCode::Invalid,
        "busy conversation rejects a second turn"
    );
    let started = next_matching(&mut rx, |e| matches!(e, SessionEvent::TurnStarted { .. })).await;
    assert!(matches!(started, SessionEvent::TurnStarted { turn: t, .. } if t == turn));
    let text = next_matching(&mut rx, |e| matches!(e, SessionEvent::AgentText { .. })).await;
    assert!(matches!(text, SessionEvent::AgentText { delta, .. } if delta == "Editing."));
    let proposal = next_matching(&mut rx, |e| matches!(e, SessionEvent::Proposal { .. })).await;
    let SessionEvent::Proposal { proposal } = proposal else {
        unreachable!()
    };
    assert_eq!(
        proposal.kind,
        analyst_protocol::transcript::ProposalKind::Edit
    );
    assert_eq!(proposal.path.as_deref(), Some("analysis/__init__.py"));
    assert_eq!(proposal.proposed.as_deref(), Some("PROPOSED = 1\n"));
    assert!(proposal
        .original
        .as_deref()
        .unwrap()
        .contains("Shared analysis library"));
    assert!(proposal.diff.as_ref().unwrap().contains("+PROPOSED = 1"));
    assert!(proposal.in_scope);
    assert_eq!(proposal.reason, ProposalReason::Review);
    assert_eq!(proposal.conversation, conv_id);

    // Accept with the user's own content.
    fx.rt
        .proposal_respond(
            &proposal.id,
            ProposalDecision::Accept {
                content: Some("USER = 2\n".into()),
            },
        )
        .unwrap();
    assert_eq!(
        fx.rt
            .proposal_respond(&proposal.id, ProposalDecision::Reject { reason: None })
            .unwrap_err()
            .code,
        IpcErrorCode::NotFound,
        "already resolved"
    );
    let resolved = next_matching(&mut rx, |e| {
        matches!(e, SessionEvent::ProposalResolved { .. })
    })
    .await;
    assert!(matches!(
        resolved,
        SessionEvent::ProposalResolved { proposal_id, decision: Decision::Accept, by: analyst_protocol::transcript::DecidedBy::User }
        if proposal_id == proposal.id
    ));
    let result = next_matching(
        &mut rx,
        |e| matches!(e, SessionEvent::AgentToolResult { id, .. } if id == "e"),
    )
    .await;
    assert!(
        matches!(result, SessionEvent::AgentToolResult { ok: true, summary, .. } if summary.contains("applied analysis/__init__.py"))
    );
    let done = next_matching(&mut rx, |e| matches!(e, SessionEvent::TurnDone { .. })).await;
    let SessionEvent::TurnDone { summary, .. } = done else {
        unreachable!()
    };
    assert_eq!(summary.text, "Done.");
    assert_eq!(summary.files_changed, vec!["analysis/__init__.py"]);
    assert_eq!(
        std::fs::read_to_string(fx.rt.repo.join("analysis/__init__.py")).unwrap(),
        "USER = 2\n"
    );
    // The model was told the user modified the proposal.
    let reqs = model.recorded();
    let tool_result = reqs[1].messages[2]
        .content
        .iter()
        .find_map(|p| match p {
            analyst_protocol::model::ContentPart::ToolResult {
                tool_call_id,
                content,
                ..
            } if tool_call_id == "e" => Some(content.clone()),
            _ => None,
        })
        .unwrap();
    assert!(tool_result.contains("user_modified"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!conv.busy());
    let info = fx.rt.info().await;
    assert_eq!(info.conversations[0].turns, 1);
    fx.wb.close_session(&fx.session).await.unwrap();
}

#[tokio::test]
async fn chat_cancel_rejects_pending_proposal() {
    let fx = fixture().await;
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            call("s", "declare_scope", json!({"files": ["scripts/new.py"]})),
            call(
                "c",
                "fs_propose_create",
                json!({"path": "scripts/new.py", "content": "x = 1\n"}),
            ),
            stop_tools(),
        ],
        vec![ModelEvent::Text {
            delta: "never".into(),
        }],
    ]));
    fx.rt.set_model_override(Some(model.clone()));
    let conv_id = fx.rt.meta().active_conversation.unwrap();
    let conv = fx.rt.conversation_runtime(&conv_id).unwrap();
    let mut rx = fx.rt.subscribe();
    agent_runner::start_turn(fx.rt.clone(), conv.clone(), "create", vec![])
        .await
        .unwrap();
    let SessionEvent::Proposal { proposal } =
        next_matching(&mut rx, |e| matches!(e, SessionEvent::Proposal { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(
        proposal.kind,
        analyst_protocol::transcript::ProposalKind::Create
    );
    assert!(proposal.original.is_none());
    agent_runner::cancel_turn(&fx.rt, &conv).await;
    let resolved = next_matching(&mut rx, |e| {
        matches!(e, SessionEvent::ProposalResolved { .. })
    })
    .await;
    assert!(matches!(
        resolved,
        SessionEvent::ProposalResolved { proposal_id, decision: Decision::Reject, .. } if proposal_id == proposal.id
    ));
    let err = next_matching(&mut rx, |e| matches!(e, SessionEvent::TurnError { .. })).await;
    assert!(matches!(err, SessionEvent::TurnError { message, .. } if message == "cancelled"));
    assert!(!conv.busy());
    assert!(!fx.rt.repo.join("scripts/new.py").exists());
    assert_eq!(
        fx.rt
            .proposal_respond(&proposal.id, ProposalDecision::Accept { content: None })
            .unwrap_err()
            .code,
        IpcErrorCode::NotFound
    );
    assert_eq!(model.calls(), 1, "the second model call never happened");
    // The conversation is usable again.
    let model2 = Arc::new(ScriptedModelClient::new(vec![vec![ModelEvent::Text {
        delta: "ok".into(),
    }]]));
    fx.rt.set_model_override(Some(model2.clone()));
    agent_runner::start_turn(fx.rt.clone(), conv.clone(), "hello", vec![])
        .await
        .unwrap();
    let done = next_matching(&mut rx, |e| matches!(e, SessionEvent::TurnDone { .. })).await;
    assert!(matches!(done, SessionEvent::TurnDone { summary, .. } if summary.text == "ok"));
    fx.wb.close_session(&fx.session).await.unwrap();
}

#[tokio::test]
async fn package_proposal_and_terminal_through_workbench() {
    let fx = fixture().await;
    // A package request shows up as a package proposal with the package list.
    let model = Arc::new(ScriptedModelClient::new(vec![
        vec![
            call("p", "pkg_add", json!({"packages": ["numpy"]})),
            stop_tools(),
        ],
        vec![ModelEvent::Text {
            delta: "fine".into(),
        }],
    ]));
    fx.rt.set_model_override(Some(model));
    let conv_id = fx.rt.meta().active_conversation.unwrap();
    let conv = fx.rt.conversation_runtime(&conv_id).unwrap();
    let mut rx = fx.rt.subscribe();
    agent_runner::start_turn(fx.rt.clone(), conv.clone(), "add numpy", vec![])
        .await
        .unwrap();
    let SessionEvent::Proposal { proposal } =
        next_matching(&mut rx, |e| matches!(e, SessionEvent::Proposal { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(
        proposal.kind,
        analyst_protocol::transcript::ProposalKind::Package
    );
    assert_eq!(
        proposal.packages.as_deref(),
        Some(&["numpy".to_string()][..])
    );
    assert_eq!(proposal.reason, ProposalReason::Ask);
    fx.rt
        .proposal_respond(
            &proposal.id,
            ProposalDecision::Reject {
                reason: Some("not now".into()),
            },
        )
        .unwrap();
    let result = next_matching(&mut rx, |e| {
        matches!(e, SessionEvent::AgentToolResult { .. })
    })
    .await;
    assert!(
        matches!(result, SessionEvent::AgentToolResult { ok: false, summary, .. } if summary.contains("not now"))
    );
    next_matching(&mut rx, |e| matches!(e, SessionEvent::TurnDone { .. })).await;

    // Terminal through the workbench: events arrive on the session channel.
    let tid = fx.wb.terminal_open(&fx.session, 80, 24).unwrap();
    fx.wb.terminal_write(&tid, "echo work-bench\n").unwrap();
    let data = next_matching(
        &mut rx,
        |e| matches!(e, SessionEvent::TerminalData { data, .. } if data.contains("work-bench")),
    )
    .await;
    assert!(matches!(data, SessionEvent::TerminalData { terminal_id, .. } if terminal_id == tid));
    fx.wb.terminal_resize(&tid, 120, 40).unwrap();
    fx.wb.terminal_close(&tid).unwrap();
    assert_eq!(
        fx.wb.terminal_write(&tid, "x").unwrap_err().code,
        IpcErrorCode::NotFound
    );
    next_matching(&mut rx, |e| matches!(e, SessionEvent::TerminalExit { .. })).await;
    fx.wb.close_session(&fx.session).await.unwrap();
}

#[tokio::test]
async fn watcher_reports_changes_and_terminal_runs() {
    let fx = fixture().await;
    let mut rx = fx.rt.subscribe();
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::write(fx.rt.repo.join("notes.md"), "# changed\n").unwrap();
    let changed = next_matching(&mut rx, |e| matches!(e, SessionEvent::FilesChanged { .. })).await;
    assert!(
        matches!(changed, SessionEvent::FilesChanged { paths } if paths.contains(&"notes.md".to_string()))
    );
    // A figure appearing under out/terminal/<stamp>/figures is a terminal run.
    let figs = fx
        .rt
        .repo
        .join("out/terminal/20260101-000000-abcdef/figures");
    std::fs::create_dir_all(&figs).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    std::fs::write(figs.join("fig-1.png"), b"\x89PNG").unwrap();
    let started = next_matching(&mut rx, |e| matches!(e, SessionEvent::RunStarted { .. })).await;
    let SessionEvent::RunStarted { run } = started else {
        unreachable!()
    };
    assert_eq!(run.meta.run_id, "terminal-20260101-000000-abcdef");
    assert_eq!(run.meta.script, "terminal");
    next_matching(&mut rx, |e| matches!(e, SessionEvent::RunDone { .. })).await;
    let art = next_matching(&mut rx, |e| matches!(e, SessionEvent::RunArtifact { .. })).await;
    assert!(
        matches!(art, SessionEvent::RunArtifact { figure: Some(f), .. } if f.path.ends_with("figures/fig-1.png"))
    );
    assert!(fx
        .rt
        .repo
        .join("out/terminal/20260101-000000-abcdef/run.json")
        .is_file());
    let groups = fx.rt.figures_list(None);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].run.meta.run_id, "terminal-20260101-000000-abcdef");
    assert_eq!(groups[0].figures.len(), 1);
    let data = fx
        .rt
        .figure_data("out/terminal/20260101-000000-abcdef/figures/fig-1.png")
        .await
        .unwrap();
    assert_eq!(data.mime, "image/png");
    assert_eq!(data.base64, "iVBORw==");
    let runs = fx.rt.runs_list(None);
    assert_eq!(runs[0].meta.run_id, "terminal-20260101-000000-abcdef");
    fx.wb.close_session(&fx.session).await.unwrap();
}

#[test]
fn models_and_targets_from_config() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        r#"
[defaults]
model = "ollama/llama"
[[providers]]
name = "ollama"
kind = "openai-compatible"
base_url = "http://localhost:11434/v1"
[[providers]]
name = "anthropic"
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
[[models]]
provider = "anthropic"
id = "claude-x"
context_window = 200000
tools = "native"
vision = true
[[targets]]
name = "lab"
kind = "ssh"
host = "lab-gpu"
data_dirs = ["/data"]
"#,
    )
    .unwrap();
    let wb = Workbench::new(WorkbenchOptions::new(home.path().to_path_buf())).unwrap();
    let models = wb.models_list();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].name, "ollama/llama");
    assert!(models[0].local);
    assert!(models[0].capabilities.is_none());
    assert_eq!(models[1].kind, ProviderKind::Anthropic);
    assert!(!models[1].local);
    assert_eq!(
        models[1].capabilities.as_ref().unwrap().context_window,
        200000
    );
    let targets = wb.targets_list();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].name, "local");
    assert_eq!(targets[1].kind, TargetKind::Ssh);
    assert_eq!(targets[1].host.as_deref(), Some("lab-gpu"));
    assert_eq!(targets[1].remote_root.as_deref(), Some("~/analyst"));
    assert_eq!(
        wb.target_bootstrap("local").unwrap_err().code,
        IpcErrorCode::Invalid
    );
    assert_eq!(
        wb.target_bootstrap("nope").unwrap_err().code,
        IpcErrorCode::NotFound
    );
    let info = wb.app_info();
    assert_eq!(info.home_abs, home.path().to_string_lossy());
}
