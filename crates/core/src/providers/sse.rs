//! Incremental Server-Sent Events parser.

/// One SSE event: the `event:` name (empty when absent) and the joined `data:` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Feed bytes as they arrive; complete events are returned from `push`.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: String,
    pending: Vec<u8>,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a chunk of bytes; returns every event completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.pending.extend_from_slice(chunk);
        // Decode as much valid UTF-8 as possible, keeping an incomplete tail.
        let valid_up_to = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) => e.valid_up_to(),
        };
        let text = String::from_utf8_lossy(&self.pending[..valid_up_to]).into_owned();
        self.pending.drain(..valid_up_to);
        self.buf.push_str(&text);
        let mut out = Vec::new();
        while let Some(idx) = self.buf.find('\n') {
            let line: String = self.buf[..idx].trim_end_matches('\r').to_string();
            self.buf.drain(..=idx);
            if let Some(ev) = self.feed_line(&line) {
                out.push(ev);
            }
        }
        out
    }

    /// Flush a trailing event that was not terminated by a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            if let Some(ev) = self.feed_line(line.trim_end_matches('\r')) {
                return Some(ev);
            }
        }
        self.dispatch()
    }

    fn feed_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_empty() {
            return None;
        }
        let ev = SseEvent {
            event: std::mem::take(&mut self.event),
            data: std::mem::take(&mut self.data).join("\n"),
        };
        Some(ev)
    }
}

/// Parse a complete SSE document (used for recorded fixtures).
pub fn parse_all(text: &str) -> Vec<SseEvent> {
    let mut p = SseParser::new();
    let mut out = p.push(text.as_bytes());
    if let Some(ev) = p.finish() {
        out.push(ev);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_across_chunks() {
        let mut p = SseParser::new();
        let mut evs = p.push(b"event: ping\ndata: {\"a\":");
        assert!(evs.is_empty());
        evs.extend(p.push(b"1}\n\n: comment\ndata: x\ndata: y\n\n"));
        assert_eq!(
            evs,
            vec![
                SseEvent {
                    event: "ping".into(),
                    data: "{\"a\":1}".into()
                },
                SseEvent {
                    event: "".into(),
                    data: "x\ny".into()
                }
            ]
        );
    }

    #[test]
    fn handles_crlf_and_split_utf8() {
        let mut p = SseParser::new();
        let text = "data: héllo\r\n\r\n";
        let bytes = text.as_bytes();
        let mut evs = p.push(&bytes[..7]); // splits inside "é"
        evs.extend(p.push(&bytes[7..]));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "héllo");
    }

    #[test]
    fn finish_flushes_trailing_event() {
        assert_eq!(parse_all("data: [DONE]").len(), 1);
    }
}
