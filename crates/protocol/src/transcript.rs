//! Transcript events, `conversations/<id>.jsonl` (PROTOCOL 5).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "lowercase")]
pub enum ProposalKind {
    Edit,
    Create,
    Delete,
    Package,
    Shell,
    Commit,
}

impl ProposalKind {
    /// Edit-class proposals: those that change files in the repo.
    pub fn is_file_change(self) -> bool {
        matches!(
            self,
            ProposalKind::Edit | ProposalKind::Create | ProposalKind::Delete
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accept,
    Reject,
    Auto,
}

impl Decision {
    pub fn is_accepted(self) -> bool {
        matches!(self, Decision::Accept | Decision::Auto)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, TS)]
#[serde(rename_all = "lowercase")]
pub enum DecidedBy {
    User,
    Policy,
}

/// One transcript line. `ts` is RFC 3339 UTC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(rename = "TranscriptEvent")]
pub enum Event {
    User {
        ts: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        attachments: Option<Vec<String>>,
    },
    Assistant {
        ts: String,
        text: String,
    },
    ToolCall {
        ts: String,
        id: String,
        name: String,
        #[ts(type = "unknown")]
        arguments: Value,
    },
    ToolResult {
        ts: String,
        id: String,
        ok: bool,
        #[ts(type = "any")]
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        truncated: Option<bool>,
    },
    Proposal {
        ts: String,
        id: String,
        kind: ProposalKind,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        diff: Option<String>,
    },
    Decision {
        ts: String,
        proposal_id: String,
        decision: Decision,
        by: DecidedBy,
    },
    Run {
        ts: String,
        run_id: String,
        script: String,
        args: Vec<String>,
        target: String,
    },
    Checkpoint {
        ts: String,
        r#ref: String,
    },
    Summary {
        ts: String,
        text: String,
        files_changed: Vec<String>,
        runs: Vec<String>,
        figures: Vec<String>,
    },
    Usage {
        ts: String,
        input_tokens: u64,
        output_tokens: u64,
        model: String,
    },
}

impl Event {
    pub fn ts(&self) -> &str {
        match self {
            Event::User { ts, .. }
            | Event::Assistant { ts, .. }
            | Event::ToolCall { ts, .. }
            | Event::ToolResult { ts, .. }
            | Event::Proposal { ts, .. }
            | Event::Decision { ts, .. }
            | Event::Run { ts, .. }
            | Event::Checkpoint { ts, .. }
            | Event::Summary { ts, .. }
            | Event::Usage { ts, .. } => ts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_round_trip_with_documented_tags() {
        let events = vec![
            Event::User {
                ts: "t".into(),
                text: "hi".into(),
                attachments: None,
            },
            Event::Assistant {
                ts: "t".into(),
                text: "yo".into(),
            },
            Event::ToolCall {
                ts: "t".into(),
                id: "c1".into(),
                name: "fs.read".into(),
                arguments: json!({"path":"a.py"}),
            },
            Event::ToolResult {
                ts: "t".into(),
                id: "c1".into(),
                ok: true,
                result: json!({"content":"x"}),
                truncated: Some(false),
            },
            Event::Proposal {
                ts: "t".into(),
                id: "p1".into(),
                kind: ProposalKind::Edit,
                summary: "edit a.py".into(),
                diff: Some("--- a\n+++ b\n".into()),
            },
            Event::Decision {
                ts: "t".into(),
                proposal_id: "p1".into(),
                decision: Decision::Auto,
                by: DecidedBy::Policy,
            },
            Event::Run {
                ts: "t".into(),
                run_id: "r".into(),
                script: "s.py".into(),
                args: vec![],
                target: "local".into(),
            },
            Event::Checkpoint {
                ts: "t".into(),
                r#ref: "refs/analyst/checkpoints/x".into(),
            },
            Event::Summary {
                ts: "t".into(),
                text: "done".into(),
                files_changed: vec!["a.py".into()],
                runs: vec![],
                figures: vec![],
            },
            Event::Usage {
                ts: "t".into(),
                input_tokens: 1,
                output_tokens: 2,
                model: "p/m".into(),
            },
        ];
        let tags = [
            "user",
            "assistant",
            "tool_call",
            "tool_result",
            "proposal",
            "decision",
            "run",
            "checkpoint",
            "summary",
            "usage",
        ];
        for (e, tag) in events.iter().zip(tags) {
            let v = serde_json::to_value(e).unwrap();
            assert_eq!(v["type"], tag);
            let back: Event = serde_json::from_value(v).unwrap();
            assert_eq!(&back, e);
        }
        let v = serde_json::to_value(&events[7]).unwrap();
        assert_eq!(v["ref"], "refs/analyst/checkpoints/x");
    }
}
