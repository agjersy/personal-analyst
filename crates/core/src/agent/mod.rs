//! The agent loop (ARCHITECTURE 5.7): context assembly, modes, a bounded tool loop with policy
//! evaluation and approvals, scope declaration, post-edit lint and contract checks with
//! bounded auto-fix, run failures fed back, turn summaries, and compaction.

pub mod approver;
pub mod prompt;

pub use approver::{
    ApprovalDecision, ApprovalRequest, Approver, AutoApprover, RecordingApprover, RejectAllApprover,
};

use crate::checkpoint;
use crate::policy::{Decision as PolicyDecision, Policy};
use crate::providers::text_protocol;
use crate::providers::ModelClient;
use crate::tools::proposals::{self, Proposal};
use crate::tools::registry::{self, ToolClass};
use crate::tools::ToolContext;
use crate::util::{estimate_tokens, now_rfc3339};
use analyst_protocol::config::ToolSupport;
use analyst_protocol::evals::Mode;
use analyst_protocol::kernel::{Diagnostic, Severity};
use analyst_protocol::model::{
    ChatRequest, ContentPart, Message, ModelCapabilities, ModelEvent, Role, StopReason,
};
use analyst_protocol::transcript::{DecidedBy, Decision, Event, ProposalKind};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub mode: Mode,
    pub max_steps: u32,
    /// Lint/contract feedback rounds per turn before surfacing to the user (N=3).
    pub auto_fix_iterations: u32,
    /// Compact when the context exceeds this fraction of the window.
    pub compaction_fraction: f32,
    /// `provider/model`, recorded in usage events.
    pub model_name: String,
    pub caps: ModelCapabilities,
    pub checkpoint_keep: usize,
    pub max_tokens: Option<u32>,
}

impl AgentConfig {
    pub fn new(mode: Mode, model_name: &str, caps: ModelCapabilities, max_steps: u32) -> Self {
        AgentConfig {
            mode,
            max_steps,
            auto_fix_iterations: 3,
            compaction_fraction: 0.7,
            model_name: model_name.to_string(),
            caps,
            checkpoint_keep: 20,
            max_tokens: Some(4096),
        }
    }
}

/// What a turn produced.
#[derive(Debug, Clone, Default)]
pub struct TurnSummary {
    pub text: String,
    pub files_changed: Vec<String>,
    pub runs: Vec<String>,
    pub figures: Vec<String>,
    pub steps: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u32,
    pub tool_errors: u32,
    /// Accepted edits outside the declared scope (or with no scope declared).
    pub unwanted_edits: u32,
    pub open_issues: Vec<String>,
    pub stopped_by_max_steps: bool,
}

pub type TextSink = Arc<dyn Fn(&str) + Send + Sync>;
pub type EventSink = Arc<dyn Fn(&Event) + Send + Sync>;

pub struct Agent {
    pub model: Arc<dyn ModelClient>,
    pub ctx: ToolContext,
    pub policy: Policy,
    pub approver: Arc<dyn Approver>,
    pub config: AgentConfig,
    history: Vec<Message>,
    pinned: Vec<String>,
    turn_counter: u32,
    project_map: String,
    pub on_text: Option<TextSink>,
    pub on_event: Option<EventSink>,
}

struct Collected {
    text: String,
    calls: Vec<(String, String, Value)>,
    input_tokens: u64,
    output_tokens: u64,
    stop: StopReason,
    message: Option<String>,
}

impl Agent {
    pub fn new(
        model: Arc<dyn ModelClient>,
        ctx: ToolContext,
        policy: Policy,
        approver: Arc<dyn Approver>,
        config: AgentConfig,
    ) -> Self {
        Agent {
            model,
            ctx,
            policy,
            approver,
            config,
            history: vec![],
            pinned: vec![],
            turn_counter: 0,
            project_map: String::new(),
            on_text: None,
            on_event: None,
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.config.mode = mode;
        self.policy.mode = mode;
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Make the history well-formed again after a turn was aborted mid-way (the desktop app
    /// cancels turns by dropping the future): every assistant tool call needs a result, and the
    /// history must end with an assistant message before the next user message.
    pub fn recover_after_cancel(&mut self) {
        if let Some(last) = self.history.last() {
            if last.role == Role::Assistant {
                let pending: Vec<String> = last
                    .content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::ToolCall { id, .. } => Some(id.clone()),
                        _ => None,
                    })
                    .collect();
                if !pending.is_empty() {
                    let parts = pending
                        .into_iter()
                        .map(|id| ContentPart::ToolResult {
                            tool_call_id: id,
                            content: "{\"error\":\"cancelled by the user\"}".into(),
                            is_error: true,
                        })
                        .collect();
                    self.history.push(Message {
                        role: Role::User,
                        content: parts,
                    });
                }
            }
        }
        if !matches!(self.history.last(), Some(m) if m.role == Role::Assistant) {
            self.history
                .push(Message::assistant_text("(turn cancelled by the user)"));
        }
    }

    pub fn pinned_facts(&self) -> &[String] {
        &self.pinned
    }

    fn uses_text_protocol(&self) -> bool {
        !matches!(self.config.caps.tools, ToolSupport::Native)
    }

    fn record(&self, event: Event) -> Result<()> {
        if let Some(cb) = &self.on_event {
            cb(&event);
        }
        self.ctx.session.append_event(&self.ctx.conv_id, &event)
    }

    fn system_prompt(&self) -> String {
        let mut s = String::new();
        s.push_str(prompt::SYSTEM_ROLE);
        s.push_str("\n\n");
        s.push_str(&prompt::contract_rules(&self.ctx.project));
        s.push_str("\n\n");
        s.push_str(prompt::OUTPUT_CONVENTIONS);
        s.push_str("\n\n");
        s.push_str(prompt::mode_instructions(self.config.mode));
        s.push_str("\n\nPOLICY: ");
        s.push_str(&self.policy.summary());
        s.push_str("\n\n");
        s.push_str(&self.project_map);
        if !self.pinned.is_empty() {
            s.push_str("\nPINNED FACTS:\n");
            for f in &self.pinned {
                s.push_str("- ");
                s.push_str(f);
                s.push('\n');
            }
        }
        if self.uses_text_protocol() {
            s.push_str("\n\n");
            s.push_str(&text_protocol::instructions(&registry::defs_for_mode(
                self.config.mode,
            )));
        }
        s
    }

    fn build_request(&self) -> ChatRequest {
        ChatRequest {
            model: self.model.model_id().to_string(),
            system: Some(self.system_prompt()),
            messages: self.history.clone(),
            tools: if self.uses_text_protocol() {
                vec![]
            } else {
                registry::defs_for_mode(self.config.mode)
            },
            max_tokens: self.config.max_tokens,
            temperature: None,
        }
    }

    async fn call_model(&self) -> Collected {
        let mut stream = self.model.stream(self.build_request());
        let mut c = Collected {
            text: String::new(),
            calls: vec![],
            input_tokens: 0,
            output_tokens: 0,
            stop: StopReason::End,
            message: None,
        };
        while let Some(ev) = stream.next().await {
            match ev {
                ModelEvent::Text { delta } => {
                    if let Some(cb) = &self.on_text {
                        cb(&delta);
                    }
                    c.text.push_str(&delta);
                }
                ModelEvent::ToolCall {
                    id,
                    name,
                    arguments,
                } => c.calls.push((id, name, arguments)),
                ModelEvent::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    c.input_tokens += input_tokens;
                    c.output_tokens += output_tokens;
                }
                ModelEvent::Stop { reason, message } => {
                    c.stop = reason;
                    c.message = message;
                    break;
                }
            }
        }
        c
    }

    /// Run one turn for a user message.
    pub async fn run_turn(&mut self, user_text: &str) -> Result<TurnSummary> {
        self.turn_counter += 1;
        let turn_id = format!("{}-{}", self.ctx.session.id(), self.turn_counter);
        let mut summary = TurnSummary::default();

        if self.config.mode == Mode::Build {
            match checkpoint::create(&self.ctx.repo, &turn_id) {
                Ok(r) => {
                    self.record(Event::Checkpoint {
                        ts: now_rfc3339(),
                        r#ref: r,
                    })?;
                    let _ = checkpoint::prune(&self.ctx.repo, self.config.checkpoint_keep);
                }
                Err(e) => tracing::warn!("checkpoint failed: {e}"),
            }
        }
        let user_changed = self
            .ctx
            .session
            .files_changed_since_last_turn()
            .unwrap_or_default();
        self.project_map = prompt::project_map(&self.ctx, &user_changed).await;

        self.record(Event::User {
            ts: now_rfc3339(),
            text: user_text.to_string(),
            attachments: None,
        })?;
        let mut text = user_text.to_string();
        if !user_changed.is_empty() {
            text.push_str(&format!(
                "\n\n[Files changed by the user since the last turn: {}]",
                user_changed.join(", ")
            ));
        }
        self.history.push(Message::user_text(text));

        let mut fix_rounds = 0u32;
        let mut files_changed: BTreeSet<String> = BTreeSet::new();
        let mut last_text = String::new();
        loop {
            if summary.steps >= self.config.max_steps {
                summary.stopped_by_max_steps = true;
                summary
                    .open_issues
                    .push(format!("stopped after max_steps={}", self.config.max_steps));
                break;
            }
            summary.steps += 1;
            self.maybe_compact().await;

            let c = self.call_model().await;
            summary.input_tokens += c.input_tokens;
            summary.output_tokens += c.output_tokens;
            self.record(Event::Usage {
                ts: now_rfc3339(),
                input_tokens: c.input_tokens,
                output_tokens: c.output_tokens,
                model: self.config.model_name.clone(),
            })?;
            if c.stop == StopReason::Error {
                let msg = c.message.clone().unwrap_or_else(|| "model error".into());
                self.record(Event::Assistant {
                    ts: now_rfc3339(),
                    text: format!("[model error] {msg}"),
                })?;
                return Err(anyhow!("model error: {msg}"));
            }

            let (clean_text, mut calls) = if self.uses_text_protocol() {
                let (t, parsed) = text_protocol::parse_tool_calls(&c.text);
                let calls = parsed
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| {
                        (
                            format!("tc-{}-{}-{i}", self.turn_counter, summary.steps),
                            p.name,
                            p.arguments,
                        )
                    })
                    .collect::<Vec<_>>();
                (t, calls)
            } else {
                (c.text.clone(), vec![])
            };
            calls.extend(c.calls.clone());

            let mut assistant_parts = Vec::new();
            if !clean_text.trim().is_empty() {
                self.record(Event::Assistant {
                    ts: now_rfc3339(),
                    text: clean_text.clone(),
                })?;
                last_text = clean_text.clone();
            }
            if self.uses_text_protocol() {
                assistant_parts.push(ContentPart::Text {
                    text: c.text.clone(),
                });
            } else if !clean_text.is_empty() {
                assistant_parts.push(ContentPart::Text {
                    text: clean_text.clone(),
                });
            }
            for (id, name, args) in &calls {
                self.record(Event::ToolCall {
                    ts: now_rfc3339(),
                    id: id.clone(),
                    name: name.clone(),
                    arguments: args.clone(),
                })?;
                if !self.uses_text_protocol() {
                    assistant_parts.push(ContentPart::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: args.clone(),
                    });
                }
            }
            if assistant_parts.is_empty() {
                assistant_parts.push(ContentPart::Text {
                    text: "(no output)".into(),
                });
            }
            self.history.push(Message {
                role: Role::Assistant,
                content: assistant_parts,
            });

            if calls.is_empty() {
                if c.stop == StopReason::MaxTokens {
                    summary
                        .open_issues
                        .push("the model hit its output token limit".into());
                }
                break;
            }

            let mut result_parts: Vec<ContentPart> = Vec::new();
            let mut edited_this_round: Vec<String> = Vec::new();
            for (id, model_name, args) in &calls {
                summary.tool_calls += 1;
                let outcome = self
                    .execute_call(id, model_name, args, &mut summary, &mut edited_this_round)
                    .await;
                self.record(Event::ToolResult {
                    ts: now_rfc3339(),
                    id: id.clone(),
                    ok: outcome.ok,
                    result: outcome.result.clone(),
                    truncated: if outcome.truncated { Some(true) } else { None },
                })?;
                if !outcome.ok {
                    summary.tool_errors += 1;
                }
                if self.uses_text_protocol() {
                    result_parts.push(ContentPart::Text {
                        text: text_protocol::format_result(model_name, &outcome.result),
                    });
                } else {
                    result_parts.push(ContentPart::ToolResult {
                        tool_call_id: id.clone(),
                        content: serde_json::to_string(&outcome.result).unwrap_or_default(),
                        is_error: !outcome.ok,
                    });
                }
            }
            for f in &edited_this_round {
                files_changed.insert(f.clone());
            }
            if !edited_this_round.is_empty() {
                if let Some(feedback) = self.post_edit_checks(&edited_this_round).await {
                    if fix_rounds < self.config.auto_fix_iterations {
                        fix_rounds += 1;
                        result_parts.push(ContentPart::Text {
                            text: format!("[auto-check round {fix_rounds}/{}] Diagnostics after your edits (fix errors before finishing):\n{feedback}", self.config.auto_fix_iterations),
                        });
                    } else {
                        summary.open_issues.push(format!(
                            "diagnostics remain after {} auto-fix rounds:\n{feedback}",
                            self.config.auto_fix_iterations
                        ));
                        result_parts.push(ContentPart::Text {
                            text: "[auto-check] Diagnostics remain but the auto-fix budget is exhausted; they will be surfaced to the user. Wrap up and report them.".into(),
                        });
                    }
                }
            }
            self.history.push(Message {
                role: Role::User,
                content: result_parts,
            });
        }

        summary.text = last_text;
        summary.files_changed = files_changed.into_iter().collect();
        if !summary.files_changed.is_empty() {
            self.pinned.push(format!(
                "Turn {}: changed {}",
                self.turn_counter,
                summary.files_changed.join(", ")
            ));
        }
        self.record(Event::Summary {
            ts: now_rfc3339(),
            text: summary.text.clone(),
            files_changed: summary.files_changed.clone(),
            runs: summary.runs.clone(),
            figures: summary.figures.clone(),
        })?;
        let _ = self.ctx.session.mark_worktree();
        Ok(summary)
    }

    async fn execute_call(
        &mut self,
        id: &str,
        model_name: &str,
        args: &Value,
        summary: &mut TurnSummary,
        edited: &mut Vec<String>,
    ) -> crate::tools::ToolOutcome {
        let Some(name) = registry::internal_name(model_name) else {
            return self.ctx.cap_result(
                id,
                Err(crate::tools::ToolError::new(format!(
                    "unknown tool `{model_name}`"
                ))),
            );
        };
        match name {
            "declare_scope" => {
                let files: Vec<String> = args
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                self.policy.declare_scope(files.clone());
                self.pinned.retain(|p| !p.starts_with("Declared scope:"));
                self.pinned
                    .push(format!("Declared scope: {}", files.join(", ")));
                if let Some(plan) = args.get("plan").and_then(Value::as_str) {
                    self.pinned
                        .push(format!("Plan: {}", crate::util::truncate_bytes(plan, 500)));
                }
                return self
                    .ctx
                    .cap_result(id, Ok(json!({"ok": true, "scope": files})));
            }
            "pin_fact" => {
                if let Some(f) = args.get("fact").and_then(Value::as_str) {
                    self.pinned.push(f.to_string());
                }
                return self.ctx.cap_result(id, Ok(json!({"ok": true})));
            }
            _ => {}
        }
        let class = registry::tool_class(name);
        let eval = self.policy.evaluate(name, args);
        let is_edit = class == Some(ToolClass::Edit);
        let mut proposal: Option<Proposal> = None;
        if is_edit && eval.decision != PolicyDecision::Deny {
            match proposals::build(&self.ctx.repo, id, name, args) {
                Ok(p) => {
                    let _ = self.record(Event::Proposal {
                        ts: now_rfc3339(),
                        id: p.id.clone(),
                        kind: p.kind,
                        summary: p.summary.clone(),
                        diff: Some(p.diff.clone()),
                    });
                    proposal = Some(p);
                }
                Err(e) => return self.ctx.cap_result(id, Err(e.into())),
            }
        }
        let pk = proposal_kind_for(name);
        let mut user_modified = false;
        match eval.decision {
            PolicyDecision::Deny => {
                if is_edit {
                    let _ = self.record(Event::Proposal {
                        ts: now_rfc3339(),
                        id: id.to_string(),
                        kind: pk,
                        summary: format!(
                            "{name} {}",
                            args.get("path").and_then(Value::as_str).unwrap_or("")
                        ),
                        diff: None,
                    });
                    let _ = self.record(Event::Decision {
                        ts: now_rfc3339(),
                        proposal_id: id.to_string(),
                        decision: Decision::Reject,
                        by: DecidedBy::Policy,
                    });
                }
                return self.ctx.cap_result(
                    id,
                    Err(crate::tools::ToolError::new(format!(
                        "denied by policy: {}",
                        eval.reason
                    ))),
                );
            }
            PolicyDecision::Ask => {
                let req = ApprovalRequest {
                    tool: name.to_string(),
                    args: args.clone(),
                    reason: eval.reason.clone(),
                    proposal: proposal.clone(),
                };
                let decision = self.approver.decide(&req).await;
                let ok = decision.accepted();
                if is_edit
                    || class == Some(ToolClass::Package)
                    || class == Some(ToolClass::Shell)
                    || class == Some(ToolClass::Git)
                {
                    if !is_edit {
                        let _ = self.record(Event::Proposal {
                            ts: now_rfc3339(),
                            id: id.to_string(),
                            kind: pk,
                            summary: format!("{name} {}", compact_args(args)),
                            diff: None,
                        });
                    }
                    let _ = self.record(Event::Decision {
                        ts: now_rfc3339(),
                        proposal_id: id.to_string(),
                        decision: if ok {
                            Decision::Accept
                        } else {
                            Decision::Reject
                        },
                        by: DecidedBy::User,
                    });
                }
                match decision {
                    ApprovalDecision::Reject { reason } => {
                        let why = match reason {
                            Some(r) if !r.trim().is_empty() => {
                                format!("{}; user says: {r}", eval.reason)
                            }
                            _ => eval.reason.clone(),
                        };
                        return self.ctx.cap_result(
                            id,
                            Err(crate::tools::ToolError::new(format!(
                                "rejected by the user ({why})"
                            ))),
                        );
                    }
                    ApprovalDecision::Accept { content } => {
                        if is_edit {
                            summary.unwanted_edits += 1;
                        }
                        if let (Some(c), Some(p)) = (content, proposal.as_mut()) {
                            if p.kind != ProposalKind::Delete {
                                p.new_content = Some(c);
                                user_modified = true;
                            }
                        }
                    }
                }
            }
            PolicyDecision::Allow => {
                if is_edit {
                    let _ = self.record(Event::Decision {
                        ts: now_rfc3339(),
                        proposal_id: id.to_string(),
                        decision: Decision::Auto,
                        by: DecidedBy::Policy,
                    });
                }
            }
        }
        if let Some(p) = proposal {
            let r = proposals::apply(&self.ctx.repo, &p).map(|_| {
                edited.push(p.path.clone());
                if user_modified {
                    json!({
                        "applied": true,
                        "path": p.path,
                        "summary": p.summary,
                        "user_modified": true,
                        "note": "the user edited your proposal before accepting it; the file on disk is their version, so re-read it before editing it again"
                    })
                } else {
                    json!({"applied": true, "path": p.path, "summary": p.summary})
                }
            });
            return self.ctx.cap_result(id, r.map_err(Into::into));
        }
        let result = self.ctx.dispatch(id, name, args).await;
        if name == "run.script" {
            if let Ok(v) = &result {
                if let Some(r) = v.get("run_id").and_then(Value::as_str) {
                    summary.runs.push(r.to_string());
                }
                if let Some(figs) = v.get("figures").and_then(Value::as_array) {
                    summary.figures.extend(
                        figs.iter()
                            .filter_map(|f| f.as_str().map(|s| s.to_string())),
                    );
                }
            }
        }
        self.ctx.cap_result(id, result)
    }

    /// Lint + contract check (including similarity) on edited files; returns feedback text when
    /// there are diagnostics. Silently skipped when the kernel is unavailable.
    async fn post_edit_checks(&self, edited: &[String]) -> Option<String> {
        let kernel = self.ctx.kernel.as_ref()?;
        let py: Vec<String> = edited
            .iter()
            .filter(|p| p.ends_with(".py"))
            .cloned()
            .collect();
        if py.is_empty() {
            return None;
        }
        let mut diags: Vec<Diagnostic> = Vec::new();
        match kernel.lint_run(Some(py.clone())).await {
            Ok(r) => diags.extend(r.diagnostics),
            Err(e) => tracing::warn!("lint.run failed: {e}"),
        }
        match kernel.contract_check(None).await {
            Ok(r) => diags.extend(
                r.diagnostics
                    .into_iter()
                    .filter(|d| py.contains(&d.path) || d.rule.starts_with("lib.")),
            ),
            Err(e) => tracing::warn!("contract.check failed: {e}"),
        }
        if diags.is_empty() {
            return None;
        }
        let mut s = String::new();
        for d in diags.iter().take(60) {
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warn => "warn",
            };
            s.push_str(&format!(
                "- {sev} {} {}:{}:{} {}",
                d.rule, d.path, d.line, d.col, d.message
            ));
            if let Some(h) = &d.hint {
                s.push_str(&format!(" (hint: {h})"));
            }
            s.push('\n');
        }
        Some(s)
    }

    fn context_tokens(&self) -> usize {
        let mut n = estimate_tokens(&self.system_prompt());
        for m in &self.history {
            n += message_tokens(m);
        }
        n
    }

    /// Summarize older turns via the model when history exceeds the configured fraction of
    /// the context window. Keeps a tail that starts at a genuine user message.
    pub async fn maybe_compact(&mut self) {
        let budget =
            (self.config.caps.context_window as f32 * self.config.compaction_fraction) as usize;
        if self.context_tokens() <= budget || self.history.len() < 4 {
            return;
        }
        // Find the split: the latest genuine user message (text only) that is not the last message,
        // leaving at least two messages before it to summarize.
        let mut split = None;
        for i in (2..self.history.len() - 1).rev() {
            let m = &self.history[i];
            if m.role == Role::User
                && m.content
                    .iter()
                    .all(|p| matches!(p, ContentPart::Text { .. }))
            {
                split = Some(i);
                break;
            }
        }
        let split = match split {
            Some(s) => s,
            None => self.history.len() - 1,
        };
        if split < 2 {
            return;
        }
        let older: Vec<Message> = self.history[..split].to_vec();
        let mut transcript = String::new();
        for m in &older {
            let role = match m.role {
                Role::User => "USER",
                Role::Assistant => "ASSISTANT",
            };
            for p in &m.content {
                match p {
                    ContentPart::Text { text } => transcript.push_str(&format!(
                        "{role}: {}\n",
                        crate::util::truncate_bytes(text, 4000)
                    )),
                    ContentPart::ToolCall {
                        name, arguments, ..
                    } => transcript.push_str(&format!(
                        "{role} CALL {name} {}\n",
                        crate::util::truncate_bytes(&arguments.to_string(), 500)
                    )),
                    ContentPart::ToolResult { content, .. } => transcript.push_str(&format!(
                        "RESULT {}\n",
                        crate::util::truncate_bytes(content, 1500)
                    )),
                    ContentPart::Image { .. } => transcript.push_str("[image]\n"),
                }
            }
        }
        let req = ChatRequest {
            model: self.model.model_id().to_string(),
            system: Some("You compact conversation history. Produce a dense summary that preserves every decision, file name, function name, parameter value, run id, error, and open question. Use short bullet points.".into()),
            messages: vec![Message::user_text(format!("Summarize this conversation segment:\n\n{transcript}"))],
            tools: vec![],
            max_tokens: Some(1500),
            temperature: Some(0.0),
        };
        let c = crate::providers::collect(self.model.stream(req)).await;
        if c.stop == Some(StopReason::Error) || c.text.trim().is_empty() {
            tracing::warn!("compaction summary failed; keeping history");
            return;
        }
        let tail: Vec<Message> = self.history[split..].to_vec();
        let mut new_history = vec![
            Message::user_text(format!(
                "[Summary of earlier conversation]\n{}",
                c.text.trim()
            )),
            Message::assistant_text("Understood; continuing from that summary."),
        ];
        new_history.extend(tail);
        self.history = new_history;
    }
}

fn message_tokens(m: &Message) -> usize {
    m.content
        .iter()
        .map(|p| match p {
            ContentPart::Text { text } => estimate_tokens(text),
            ContentPart::Image { data, .. } => data.len() / 4 / 3,
            ContentPart::ToolCall { arguments, .. } => estimate_tokens(&arguments.to_string()) + 10,
            ContentPart::ToolResult { content, .. } => estimate_tokens(content) + 10,
        })
        .sum::<usize>()
        + 4
}

fn proposal_kind_for(tool: &str) -> ProposalKind {
    match tool {
        "fs.propose_create" => ProposalKind::Create,
        "fs.propose_delete" => ProposalKind::Delete,
        "pkg.add" | "pkg.remove" | "pkg.sync" => ProposalKind::Package,
        "shell.run" => ProposalKind::Shell,
        "git.commit" => ProposalKind::Commit,
        _ => ProposalKind::Edit,
    }
}

fn compact_args(args: &Value) -> String {
    crate::util::truncate_bytes(&args.to_string(), 200).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ScriptedModelClient;
    use crate::session::Session;
    use analyst_protocol::config::{EditPolicy, PolicyConfig, ProjectConfig};

    fn setup(
        edits: EditPolicy,
        mode: Mode,
    ) -> (tempfile::TempDir, tempfile::TempDir, ToolContext, Policy) {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        checkpoint::git(repo.path(), &["init", "-q"], None).unwrap();
        std::fs::create_dir_all(repo.path().join("scripts")).unwrap();
        std::fs::write(repo.path().join("scripts/a.py"), "print('hello')\n").unwrap();
        checkpoint::git(repo.path(), &["add", "-A"], None).unwrap();
        checkpoint::git(repo.path(), &["commit", "-q", "-m", "init"], None).unwrap();
        let session = Session::create(home.path(), repo.path(), "local", "scripted/model").unwrap();
        let ctx = ToolContext::new(
            repo.path(),
            home.path(),
            session,
            "c1",
            ProjectConfig::default(),
        );
        let policy = Policy::new(
            repo.path(),
            PolicyConfig {
                edits,
                ..Default::default()
            },
            mode,
        )
        .unwrap();
        (home, repo, ctx, policy)
    }

    fn agent(
        model: Arc<ScriptedModelClient>,
        ctx: ToolContext,
        policy: Policy,
        approver: Arc<dyn Approver>,
        max_steps: u32,
    ) -> Agent {
        let mode = policy.mode;
        Agent::new(
            model,
            ctx,
            policy,
            approver,
            AgentConfig::new(
                mode,
                "scripted/model",
                ModelCapabilities::default(),
                max_steps,
            ),
        )
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

    #[tokio::test]
    async fn tool_call_is_executed_and_result_appended() {
        let (_h, _r, ctx, policy) = setup(EditPolicy::Review, Mode::Plan);
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                ModelEvent::Text {
                    delta: "Reading.".into(),
                },
                call("c1", "fs_read", json!({"path": "scripts/a.py"})),
                stop_tools(),
            ],
            vec![
                ModelEvent::Text {
                    delta: "Done.".into(),
                },
                ModelEvent::Usage {
                    input_tokens: 5,
                    output_tokens: 2,
                },
            ],
        ]));
        let mut a = agent(model.clone(), ctx, policy, Arc::new(RejectAllApprover), 10);
        let s = a.run_turn("read a.py").await.unwrap();
        assert_eq!(s.steps, 2);
        assert_eq!(s.tool_calls, 1);
        assert_eq!(s.tool_errors, 0);
        assert_eq!(s.text, "Done.");
        assert_eq!(s.input_tokens, 5);
        let reqs = model.recorded();
        assert_eq!(reqs.len(), 2);
        let second = &reqs[1];
        assert!(second.system.as_ref().unwrap().contains("SCRIPT CONTRACT"));
        assert!(!second.tools.is_empty());
        let asst = &second.messages[1];
        assert_eq!(asst.role, Role::Assistant);
        assert!(
            matches!(&asst.content[1], ContentPart::ToolCall { name, .. } if name == "fs_read")
        );
        let res = &second.messages[2];
        assert_eq!(res.role, Role::User);
        match &res.content[0] {
            ContentPart::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_call_id, "c1");
                assert!(!is_error);
                assert!(content.contains("hello"));
            }
            other => panic!("unexpected {other:?}"),
        }
        let events = a.ctx.session.read_transcript("c1").unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::ToolCall { name, .. } if name == "fs_read")));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::ToolResult { ok: true, .. })));
        assert!(matches!(events.last().unwrap(), Event::Summary { .. }));
        assert!(
            !events.iter().any(|e| matches!(e, Event::Checkpoint { .. })),
            "plan mode makes no checkpoint"
        );
    }

    #[tokio::test]
    async fn review_mode_pauses_and_rejection_fails_cleanly() {
        let (_h, repo, ctx, policy) = setup(EditPolicy::Review, Mode::Build);
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                call("s", "declare_scope", json!({"files": ["scripts/b.py"]})),
                call(
                    "e",
                    "fs_propose_create",
                    json!({"path": "scripts/b.py", "content": "x = 1\n"}),
                ),
                stop_tools(),
            ],
            vec![ModelEvent::Text {
                delta: "Rejected, stopping.".into(),
            }],
        ]));
        let mut a = agent(model.clone(), ctx, policy, Arc::new(RejectAllApprover), 10);
        let s = a.run_turn("create b.py").await.unwrap();
        assert!(!repo.path().join("scripts/b.py").exists());
        assert_eq!(s.tool_errors, 1);
        assert_eq!(s.unwanted_edits, 0);
        assert!(s.files_changed.is_empty());
        let reqs = model.recorded();
        let res = &reqs[1].messages[2];
        let err = res
            .content
            .iter()
            .find_map(|p| match p {
                ContentPart::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                } if tool_call_id == "e" => Some((content.clone(), *is_error)),
                _ => None,
            })
            .unwrap();
        assert!(err.1);
        assert!(err.0.contains("rejected by the user"));
        let events = a.ctx.session.read_transcript("c1").unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::Checkpoint { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Proposal {
                kind: ProposalKind::Create,
                ..
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Decision {
                decision: Decision::Reject,
                by: DecidedBy::User,
                ..
            }
        )));
        assert!(!checkpoint::list(repo.path()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn out_of_scope_edit_under_auto_asks() {
        let (_h, repo, ctx, policy) = setup(EditPolicy::Auto, Mode::Build);
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                call(
                    "s",
                    "declare_scope",
                    json!({"files": ["scripts/a.py"], "plan": "tweak a"}),
                ),
                stop_tools(),
            ],
            vec![
                call(
                    "in",
                    "fs_propose_edit",
                    json!({"path": "scripts/a.py", "edits": [{"old": "hello", "new": "bye"}]}),
                ),
                call(
                    "out",
                    "fs_propose_create",
                    json!({"path": "scripts/c.py", "content": "c\n"}),
                ),
                stop_tools(),
            ],
            vec![ModelEvent::Text { delta: "ok".into() }],
        ]));
        let approver = Arc::new(RecordingApprover::new(true));
        let mut a = agent(model.clone(), ctx, policy, approver.clone(), 10);
        let s = a.run_turn("edit").await.unwrap();
        let asked = approver.seen.lock().unwrap();
        assert_eq!(asked.len(), 1, "only the out-of-scope edit asks");
        assert_eq!(asked[0].tool, "fs.propose_create");
        assert!(asked[0].reason.contains("outside the declared scope"));
        assert!(asked[0].proposal.as_ref().unwrap().diff.contains("+c"));
        assert_eq!(
            std::fs::read_to_string(repo.path().join("scripts/a.py")).unwrap(),
            "print('bye')\n"
        );
        assert!(repo.path().join("scripts/c.py").is_file());
        assert_eq!(s.unwanted_edits, 1);
        assert_eq!(s.files_changed, vec!["scripts/a.py", "scripts/c.py"]);
        let events = a.ctx.session.read_transcript("c1").unwrap();
        let decisions: Vec<(Decision, DecidedBy)> = events
            .iter()
            .filter_map(|e| match e {
                Event::Decision { decision, by, .. } => Some((*decision, *by)),
                _ => None,
            })
            .collect();
        assert_eq!(
            decisions,
            vec![
                (Decision::Auto, DecidedBy::Policy),
                (Decision::Accept, DecidedBy::User)
            ]
        );
        assert!(a
            .pinned_facts()
            .iter()
            .any(|f| f.starts_with("Declared scope: scripts/a.py")));
        assert!(model.recorded()[1]
            .system
            .as_ref()
            .unwrap()
            .contains("Declared scope: scripts/a.py"));
    }

    #[tokio::test]
    async fn max_steps_bounds_the_loop() {
        let (_h, _r, ctx, policy) = setup(EditPolicy::Review, Mode::Plan);
        let scripts: Vec<Vec<ModelEvent>> = (0..20)
            .map(|i| vec![call(&format!("c{i}"), "fs_list", json!({})), stop_tools()])
            .collect();
        let model = Arc::new(ScriptedModelClient::new(scripts));
        let mut a = agent(model.clone(), ctx, policy, Arc::new(RejectAllApprover), 3);
        let s = a.run_turn("loop").await.unwrap();
        assert_eq!(s.steps, 3);
        assert!(s.stopped_by_max_steps);
        assert_eq!(model.calls(), 3);
        assert!(s.open_issues.iter().any(|i| i.contains("max_steps")));
    }

    #[tokio::test]
    async fn text_protocol_fallback_parses_fenced_calls() {
        let (_h, _r, ctx, policy) = setup(EditPolicy::Review, Mode::Plan);
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![ModelEvent::Text { delta: "Let me look.\n```tool_call\n{\"name\": \"fs_read\", \"arguments\": {\"path\": \"scripts/a.py\"}}\n```\n".into() }],
            vec![ModelEvent::Text { delta: "It prints hello.".into() }],
        ]));
        let mut a = agent(model.clone(), ctx, policy, Arc::new(RejectAllApprover), 10);
        a.config.caps.tools = ToolSupport::TextProtocol;
        let s = a.run_turn("what does a.py do").await.unwrap();
        assert_eq!(s.tool_calls, 1);
        assert_eq!(s.tool_errors, 0);
        assert_eq!(s.text, "It prints hello.");
        let reqs = model.recorded();
        assert!(reqs[0].tools.is_empty());
        assert!(reqs[0].system.as_ref().unwrap().contains("```tool_call"));
        let res = &reqs[1].messages[2];
        match &res.content[0] {
            ContentPart::Text { text } => {
                assert!(text.starts_with("tool_result fs_read:"));
                assert!(text.contains("hello"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn denied_tool_returns_error_result() {
        let (_h, repo, ctx, policy) = setup(EditPolicy::Auto, Mode::Run);
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![
                call(
                    "e",
                    "fs_propose_create",
                    json!({"path": "scripts/z.py", "content": "z\n"}),
                ),
                stop_tools(),
            ],
            vec![ModelEvent::Text {
                delta: "cannot".into(),
            }],
        ]));
        let mut a = agent(model.clone(), ctx, policy, Arc::new(AutoApprover), 5);
        let s = a.run_turn("make z").await.unwrap();
        assert_eq!(s.tool_errors, 1);
        assert!(!repo.path().join("scripts/z.py").exists());
        let events = a.ctx.session.read_transcript("c1").unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Decision {
                decision: Decision::Reject,
                by: DecidedBy::Policy,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn compaction_summarizes_older_history() {
        let (_h, _r, ctx, policy) = setup(EditPolicy::Review, Mode::Plan);
        let long = "x".repeat(2000);
        let model = Arc::new(ScriptedModelClient::new(vec![
            vec![ModelEvent::Text {
                delta: long.clone(),
            }],
            vec![ModelEvent::Text {
                delta: long.clone(),
            }],
            vec![ModelEvent::Text {
                delta: "SUMMARY-TEXT".into(),
            }],
            vec![ModelEvent::Text {
                delta: "final".into(),
            }],
        ]));
        let mut a = agent(model.clone(), ctx, policy, Arc::new(RejectAllApprover), 5);
        a.config.caps.context_window = 1000;
        a.run_turn("one").await.unwrap();
        a.run_turn("two").await.unwrap();
        a.run_turn("three").await.unwrap();
        let reqs = model.recorded();
        assert_eq!(reqs.len(), 4);
        assert!(reqs[2].messages[0]
            .text()
            .contains("Summarize this conversation segment"));
        let last = &reqs[3];
        assert!(last.messages[0]
            .text()
            .starts_with("[Summary of earlier conversation]\nSUMMARY-TEXT"));
        assert!(last.messages.iter().any(|m| m.text() == "three"));
        assert!(a.history().len() < 7);
    }
}
