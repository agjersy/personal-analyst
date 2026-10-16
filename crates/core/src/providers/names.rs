//! Model-facing tool name mapping (PROTOCOL 4): `.` to `_`, `mcp.<server>.<tool>` to
//! `mcp__<server>__<tool>`.

/// Internal id to model-facing name.
pub fn to_model_name(internal: &str) -> String {
    if let Some(rest) = internal.strip_prefix("mcp.") {
        return format!("mcp__{}", rest.replacen('.', "__", 1));
    }
    internal.replace('.', "_")
}

/// Model-facing name to internal id, given the set of known internal ids. The reverse of
/// `.` to `_` is ambiguous in general, so the lookup is by equality of mapped names.
pub fn to_internal_name<'a>(model_name: &str, known: &[&'a str]) -> Option<&'a str> {
    known
        .iter()
        .copied()
        .find(|k| to_model_name(k) == model_name)
}

/// Whether a model-facing name is valid for providers (`^[a-zA-Z0-9_-]{1,64}$`).
pub fn is_valid_model_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        assert_eq!(to_model_name("fs.read"), "fs_read");
        assert_eq!(to_model_name("fs.propose_edit"), "fs_propose_edit");
        assert_eq!(to_model_name("contract.check"), "contract_check");
        assert_eq!(to_model_name("mcp.wolfram.query"), "mcp__wolfram__query");
        assert_eq!(
            to_internal_name("fs_propose_edit", &["fs.read", "fs.propose_edit"]),
            Some("fs.propose_edit")
        );
        assert!(is_valid_model_name("fs_read"));
        assert!(!is_valid_model_name("fs.read"));
    }
}
