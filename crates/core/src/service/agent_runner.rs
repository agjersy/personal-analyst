//! One agent turn per `chat_send`, run in a tokio task on the existing `agent` module. The
//! `UiApprover` turns policy `Ask` decisions into `proposal` events and parks on a oneshot that
//! `proposal_respond` resolves; `chat_cancel` aborts the task and rejects pending proposals.

use super::error::{ServiceError, ServiceResult};
use super::runtime::{PendingProposal, SessionRuntime};
use crate::agent::{Agent, AgentConfig, ApprovalDecision, ApprovalRequest, Approver};
use crate::config;
use crate::policy::Policy;
use crate::providers::{self, capabilities, ModelClient};
use crate::tools::ToolContext;
use crate::util::{now_rfc3339, truncate_bytes};
use analyst_protocol::evals::Mode;
use analyst_protocol::model::ModelCapabilities;
use analyst_protocol::transcript::{DecidedBy, Event, ProposalKind};
use analyst_protocol::ui::*;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use tokio::task::JoinHandle;

/// Per-conversation state: the long-lived agent (history), the busy flag, the running turn.
pub struct ConversationRuntime {
    pub id: String,
    agent: tokio::sync::Mutex<Option<Agent>>,
    busy: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
    turns: AtomicU32,
    /// Set when the model changed; the next turn rebuilds the client.
    model_dirty: AtomicBool,
}

impl ConversationRuntime {
    pub fn new(id: &str) -> Self {
        ConversationRuntime {
            id: id.to_string(),
            agent: tokio::sync::Mutex::new(None),
            busy: AtomicBool::new(false),
            task: Mutex::new(None),
            turns: AtomicU32::new(0),
            model_dirty: AtomicBool::new(false),
        }
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub async fn set_mode(&self, mode: Mode) {
        if let Some(a) = self.agent.lock().await.as_mut() {
            a.set_mode(mode);
        }
    }

    pub async fn invalidate_model(&self) {
        self.model_dirty.store(true, Ordering::SeqCst);
    }

    /// Abort the running turn, if any. Returns whether one was running.
    pub fn abort(&self) -> bool {
        let h = self.task.lock().ok().and_then(|mut t| t.take());
        match h {
            Some(h) => {
                h.abort();
                self.busy.store(false, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }
}

/// Which `ProposalReason` a policy reason string means.
fn classify_reason(reason: &str) -> (bool, ProposalReason) {
    let r = reason.to_ascii_lowercase();
    if r.contains("outside the declared scope") || r.contains("no scope declared") {
        (false, ProposalReason::OutOfScope)
    } else if r.contains("protected") {
        (false, ProposalReason::Protected)
    } else if r.contains("review") || r.contains("auto-new-files") {
        (true, ProposalReason::Review)
    } else {
        (true, ProposalReason::Ask)
    }
}

fn str_list(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect()
    })
}

/// Build the UI card for an approval request.
pub fn proposal_view(
    repo: &std::path::Path,
    id: &str,
    conversation: &str,
    req: &ApprovalRequest,
) -> ProposalView {
    let (in_scope, reason) = classify_reason(&req.reason);
    let args = &req.args;
    let mut view = ProposalView {
        id: id.to_string(),
        conversation: conversation.to_string(),
        kind: ProposalKind::Shell,
        path: None,
        summary: String::new(),
        original: None,
        proposed: None,
        diff: None,
        command: None,
        packages: None,
        message: None,
        in_scope,
        reason,
    };
    if let Some(p) = &req.proposal {
        view.kind = p.kind;
        view.path = Some(p.path.clone());
        view.summary = p.summary.clone();
        view.original = if p.kind == ProposalKind::Create {
            None
        } else {
            std::fs::read_to_string(repo.join(&p.path)).ok()
        };
        view.proposed = p.new_content.clone();
        view.diff = Some(p.diff.clone());
        return view;
    }
    match req.tool.as_str() {
        "pkg.add" | "pkg.remove" | "pkg.sync" => {
            view.kind = ProposalKind::Package;
            view.packages = Some(str_list(args.get("packages")).unwrap_or_default());
            view.summary = format!(
                "{} {}",
                req.tool,
                view.packages
                    .as_ref()
                    .map(|p| p.join(" "))
                    .unwrap_or_default()
            )
            .trim()
            .to_string();
        }
        "shell.run" => {
            view.kind = ProposalKind::Shell;
            view.command = args
                .get("command")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
            view.summary = format!("shell: {}", view.command.clone().unwrap_or_default());
        }
        "git.commit" => {
            view.kind = ProposalKind::Commit;
            view.message = args
                .get("message")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
            view.summary = format!("git commit: {}", view.message.clone().unwrap_or_default());
        }
        "run.script" => {
            view.kind = ProposalKind::Shell;
            let script = args.get("script").and_then(Value::as_str).unwrap_or("");
            let a = str_list(args.get("args")).unwrap_or_default();
            let cmd = format!("python {script} {}", a.join(" "))
                .trim()
                .to_string();
            view.path = Some(script.to_string());
            view.command = Some(cmd.clone());
            view.summary = if args
                .get("network")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                format!("run (with network): {cmd}")
            } else {
                format!("run: {cmd}")
            };
        }
        "exec.snippet" => {
            view.kind = ProposalKind::Shell;
            let code = args.get("code").and_then(Value::as_str).unwrap_or("");
            view.command = Some(format!("python -c {code}"));
            view.proposed = Some(code.to_string());
            view.summary = format!(
                "snippet: {}",
                truncate_bytes(code.lines().next().unwrap_or(""), 80)
            );
        }
        other => {
            view.summary = format!("{other} {}", truncate_bytes(&args.to_string(), 160));
            view.command = Some(args.to_string());
        }
    }
    view
}

/// Approver that asks the user through the session channel.
pub struct UiApprover {
    rt: Weak<SessionRuntime>,
    conversation: String,
}

impl UiApprover {
    pub fn new(rt: &Arc<SessionRuntime>, conversation: &str) -> Self {
        UiApprover {
            rt: Arc::downgrade(rt),
            conversation: conversation.to_string(),
        }
    }
}

#[async_trait]
impl Approver for UiApprover {
    async fn approve(&self, req: &ApprovalRequest) -> bool {
        self.decide(req).await.accepted()
    }

    async fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision {
        let Some(rt) = self.rt.upgrade() else {
            return ApprovalDecision::Reject {
                reason: Some("session closed".into()),
            };
        };
        let id = format!("p-{}", crate::util::stamped_id());
        let view = proposal_view(&rt.repo, &id, &self.conversation, req);
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Ok(mut p) = rt.pending.lock() {
            p.insert(
                id.clone(),
                PendingProposal {
                    conversation: self.conversation.clone(),
                    tx,
                },
            );
        }
        rt.emit(SessionEvent::Proposal { proposal: view });
        let decision = match rx.await {
            Ok(d) => d,
            Err(_) => ProposalDecision::Reject {
                reason: Some("cancelled".into()),
            },
        };
        let (td, result) = match decision {
            ProposalDecision::Accept { content } => (
                analyst_protocol::transcript::Decision::Accept,
                ApprovalDecision::Accept { content },
            ),
            ProposalDecision::Reject { reason } => (
                analyst_protocol::transcript::Decision::Reject,
                ApprovalDecision::Reject { reason },
            ),
        };
        rt.emit(SessionEvent::ProposalResolved {
            proposal_id: id,
            decision: td,
            by: DecidedBy::User,
        });
        result
    }
}

/// Model client + capabilities for a `provider/id` spec (capabilities from the cache, probed
/// and cached when missing).
pub async fn model_for(
    home: &std::path::Path,
    global: &analyst_protocol::config::GlobalConfig,
    spec: &str,
) -> ServiceResult<(Arc<dyn ModelClient>, ModelCapabilities, String)> {
    let spec = config::ModelSpec::parse(spec).map_err(|e| ServiceError::invalid(e.to_string()))?;
    let resolved =
        config::resolve_model(global, &spec).map_err(|e| ServiceError::not_found(e.to_string()))?;
    let client =
        providers::client_for(&resolved).map_err(|e| ServiceError::invalid(format!("{e:#}")))?;
    let key = spec.to_string();
    let caps = capabilities::ensure(
        home,
        &key,
        client.as_ref(),
        resolved.overrides.as_ref(),
        None,
        false,
    )
    .await
    .map_err(|e| ServiceError::internal(format!("{e:#}")))?;
    Ok((client, caps, key))
}

/// Append attachments to the user text the way the agent sees them.
fn render_attachments(repo: &std::path::Path, text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let mut s = text.to_string();
    for a in attachments {
        s.push_str("\n\n");
        match a {
            Attachment::File { path } => {
                let content = std::fs::read_to_string(repo.join(path)).unwrap_or_default();
                let body = truncate_bytes(&content, analyst_protocol::TOOL_RESULT_CAP_BYTES);
                s.push_str(&format!("[Attached file {path}]\n```\n{body}\n```"));
                if body.len() < content.len() {
                    s.push_str("\n(truncated; read the rest with fs.read)");
                }
            }
            Attachment::Selection {
                path,
                start_line,
                end_line,
                text,
            } => {
                s.push_str(&format!(
                    "[Selection from {path} lines {start_line}-{end_line}]\n```\n{text}\n```"
                ));
            }
            Attachment::Figure { path } => {
                let sidecar = std::fs::read_to_string(repo.join(path).with_extension("json"))
                    .unwrap_or_default();
                s.push_str(&format!("[Attached figure {path}]"));
                if !sidecar.is_empty() {
                    s.push_str(&format!(
                        "\nSidecar metadata:\n{}",
                        truncate_bytes(&sidecar, 2000)
                    ));
                }
            }
        }
    }
    s
}

/// One-line summary of a tool result for `agent_tool_result`.
fn summarize_result(ok: bool, result: &Value, truncated: bool) -> String {
    let s = if !ok {
        result
            .get("error")
            .and_then(Value::as_str)
            .map(|e| e.to_string())
            .unwrap_or_else(|| result.to_string())
    } else if let Some(p) = result.get("path").and_then(Value::as_str) {
        if result
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            format!("applied {p}")
        } else {
            result.to_string()
        }
    } else if let Some(r) = result.get("run_id").and_then(Value::as_str) {
        format!(
            "run {r} {} exit={}",
            result.get("status").and_then(Value::as_str).unwrap_or(""),
            result
                .get("exit_code")
                .map(|v| v.to_string())
                .unwrap_or_default()
        )
    } else {
        result.to_string()
    };
    let one: String = s.lines().next().unwrap_or("").to_string();
    let mut t = truncate_bytes(&one, 200).to_string();
    if truncated {
        t.push_str(" (truncated)");
    }
    t
}

/// Start a turn. Returns the turn id immediately; events follow on the session channel.
pub async fn start_turn(
    rt: Arc<SessionRuntime>,
    conv: Arc<ConversationRuntime>,
    text: &str,
    attachments: Vec<Attachment>,
) -> ServiceResult<TurnId> {
    if rt.is_closed() {
        return Err(ServiceError::invalid("session is closed"));
    }
    if conv
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(ServiceError::invalid("the conversation is busy"));
    }
    let n = conv.turns.fetch_add(1, Ordering::SeqCst) + 1;
    let turn_id = format!("{}-{n}", conv.id);
    let conv_id = conv.id.clone();
    let user_text = render_attachments(&rt.repo, text, &attachments);
    rt.emit(SessionEvent::TurnStarted {
        conversation: conv_id.clone(),
        turn: turn_id.clone(),
    });
    let tid = turn_id.clone();
    let c2 = conv.clone();
    let handle = tokio::spawn(async move {
        let outcome = run_turn(&rt, &c2, &tid, &user_text).await;
        match outcome {
            Ok(summary) => rt.emit(SessionEvent::TurnDone {
                conversation: conv_id.clone(),
                turn: tid.clone(),
                summary,
            }),
            Err(e) => rt.emit(SessionEvent::TurnError {
                conversation: conv_id.clone(),
                turn: tid.clone(),
                message: e.to_string(),
            }),
        }
        rt.reject_pending(Some(&conv_id));
        c2.busy.store(false, Ordering::SeqCst);
        if let Ok(mut t) = c2.task.lock() {
            *t = None;
        }
    });
    if let Ok(mut t) = conv.task.lock() {
        *t = Some(handle);
    }
    Ok(turn_id)
}

async fn run_turn(
    rt: &Arc<SessionRuntime>,
    conv: &Arc<ConversationRuntime>,
    turn_id: &str,
    user_text: &str,
) -> ServiceResult<TurnSummaryView> {
    let meta = rt.conversation_meta(&conv.id)?;
    let global = rt.global();
    let override_client = rt.model_override();
    let model_spec = meta
        .model
        .clone()
        .or_else(|| rt.summary().model)
        .or_else(|| global.defaults.model.clone())
        .or_else(|| override_client.as_ref().map(|c| c.model_id().to_string()))
        .ok_or_else(|| ServiceError::invalid("no model configured for this conversation"))?;
    let project = rt.effective_project();
    let mut guard = conv.agent.lock().await;
    let kernel = rt.kernel_client().await;
    let target = rt.target();
    let max_steps = project.policy.max_steps;
    let needs_new = guard.is_none();
    if needs_new {
        let (client, caps, name) = match &override_client {
            Some(c) => (c.clone(), ModelCapabilities::default(), model_spec.clone()),
            None => model_for(&rt.home, &global, &model_spec).await?,
        };
        let session = rt.session_snapshot();
        let mut ctx = ToolContext::new(&rt.repo, &rt.home, session, &conv.id, project.clone());
        ctx.kernel = kernel.clone();
        ctx.target = target.clone();
        ctx.control_dir = rt.opts.control_dir.clone();
        let policy = Policy::new(&rt.repo, project.policy.clone(), meta.mode)
            .map_err(|e| ServiceError::invalid(e.to_string()))?;
        let approver: Arc<dyn Approver> = Arc::new(UiApprover::new(rt, &conv.id));
        let config = AgentConfig::new(meta.mode, &name, caps, max_steps);
        *guard = Some(Agent::new(client, ctx, policy, approver, config));
        conv.model_dirty.store(false, Ordering::SeqCst);
    }
    let agent = guard
        .as_mut()
        .ok_or_else(|| ServiceError::internal("agent missing"))?;
    if let Some(c) = &override_client {
        agent.model = c.clone();
    } else if conv.model_dirty.swap(false, Ordering::SeqCst)
        && agent.config.model_name != model_spec
    {
        let (client, caps, name) = model_for(&rt.home, &global, &model_spec).await?;
        agent.model = client;
        agent.config.caps = caps;
        agent.config.model_name = name;
    }
    // Refresh per-turn state: kernel, target, policy (session overrides), mode.
    agent.ctx.kernel = kernel;
    agent.ctx.target = target;
    agent.ctx.project = project.clone();
    let scope = agent.policy.scope.clone();
    let mut policy = Policy::new(&rt.repo, project.policy.clone(), meta.mode)
        .map_err(|e| ServiceError::invalid(e.to_string()))?;
    policy.scope = scope;
    agent.policy = policy;
    agent.set_mode(meta.mode);
    agent.config.max_steps = max_steps;

    let events = rt.events();
    let (conv_id, tid) = (conv.id.clone(), turn_id.to_string());
    agent.on_text = Some(Arc::new(move |delta: &str| {
        let _ = events.send(SessionEvent::AgentText {
            conversation: conv_id.clone(),
            turn: tid.clone(),
            delta: delta.to_string(),
        });
    }));
    let events = rt.events();
    let (conv_id, tid) = (conv.id.clone(), turn_id.to_string());
    let context_window = agent.config.caps.context_window as u64;
    let weak = Arc::downgrade(rt);
    agent.on_event = Some(Arc::new(move |e: &Event| match e {
        Event::ToolCall {
            id,
            name,
            arguments,
            ..
        } => {
            let _ = events.send(SessionEvent::AgentToolCall {
                conversation: conv_id.clone(),
                turn: tid.clone(),
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            });
        }
        Event::ToolResult {
            id,
            ok,
            result,
            truncated,
            ..
        } => {
            let _ = events.send(SessionEvent::AgentToolResult {
                conversation: conv_id.clone(),
                turn: tid.clone(),
                id: id.clone(),
                ok: *ok,
                summary: summarize_result(*ok, result, truncated.unwrap_or(false)),
            });
            // Accepted (or auto-applied) edits: refresh diagnostics for the file.
            if *ok
                && result
                    .get("applied")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                if let (Some(path), Some(rt)) =
                    (result.get("path").and_then(Value::as_str), weak.upgrade())
                {
                    if path.ends_with(".py") {
                        let p = path.to_string();
                        tokio::spawn(async move {
                            if rt.kernel_client().await.is_some() {
                                let _ = rt.check_run(Some(vec![p])).await;
                            }
                        });
                    }
                }
            }
        }
        Event::Usage {
            input_tokens,
            output_tokens,
            ..
        } => {
            let _ = events.send(SessionEvent::Usage {
                conversation: conv_id.clone(),
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                context_window,
                context_used: *input_tokens,
            });
        }
        Event::Decision {
            proposal_id,
            decision,
            by,
            ..
        } if *by == DecidedBy::Policy => {
            let _ = events.send(SessionEvent::ProposalResolved {
                proposal_id: proposal_id.clone(),
                decision: *decision,
                by: *by,
            });
        }
        _ => {}
    }));

    let result = agent.run_turn(user_text).await;
    agent.on_text = None;
    agent.on_event = None;
    match result {
        Ok(s) => Ok(TurnSummaryView {
            text: s.text,
            files_changed: s.files_changed,
            runs: s.runs,
            figures: s.figures,
        }),
        Err(e) => Err(ServiceError::internal(format!("{e:#}"))),
    }
}

/// Abort the running turn, reject its pending proposals, and repair the agent history.
pub async fn cancel_turn(rt: &Arc<SessionRuntime>, conv: &Arc<ConversationRuntime>) {
    let was_running = conv.abort();
    rt.reject_pending(Some(&conv.id));
    if was_running {
        // The aborted task dropped its guard; repair the history for the next turn.
        if let Some(a) = conv.agent.lock().await.as_mut() {
            a.recover_after_cancel();
        }
        let turn = format!("{}-{}", conv.id, conv.turns.load(Ordering::SeqCst));
        let _ = rt.session_snapshot().append_event(
            &conv.id,
            &Event::Assistant {
                ts: now_rfc3339(),
                text: "[turn cancelled by the user]".into(),
            },
        );
        rt.emit(SessionEvent::TurnError {
            conversation: conv.id.clone(),
            turn,
            message: "cancelled".into(),
        });
    }
}
