//! Session store records under `~/.analyst/sessions/<id>/` (PROTOCOL 2.3).

use crate::kernel::RunMeta;
use serde::{Deserialize, Serialize};

/// `session.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    /// Absolute path of the local repo.
    pub repo: String,
    /// Target name (`local` or a configured SSH target).
    pub target: String,
    /// `"<provider>/<model id>"`.
    pub model: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tree_hash: Option<String>,
}

/// One line of `runs.jsonl`: a RunMeta mirror with the target added.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRun {
    #[serde(flatten)]
    pub meta: RunMeta,
    pub target: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::RunStatus;

    #[test]
    fn session_run_flattens_meta() {
        let r = SessionRun {
            meta: RunMeta {
                run_id: "r1".into(),
                script: "scripts/a.py".into(),
                args: vec![],
                status: RunStatus::Done,
                exit_code: Some(0),
                started_at: "2026-10-16T00:00:00Z".into(),
                finished_at: Some("2026-10-16T00:00:01Z".into()),
                out_dir: "out/a/r1".into(),
                sandbox: "none".into(),
                tree_hash: None,
                pid: None,
                host: "h".into(),
            },
            target: "local".into(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["run_id"], "r1");
        assert_eq!(v["target"], "local");
        let back: SessionRun = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}
