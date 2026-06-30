//! Streaming JSON framer for agent-CLI stdout.
//!
//! The external agent CLIs (Claude Code, Codex, Gemini) emit their event feed as
//! JSON — usually newline-delimited (NDJSON), one object per line. But a naive
//! `lines()` split is wrong: JSON string values can legitimately contain literal
//! `\n` (Codex's `aggregated_output`, multi-line diffs and command output), so a
//! newline is *not* an authoritative record separator. [`CliFramer`] instead
//! feeds the accumulated bytes through `serde_json`'s streaming deserializer and
//! yields each *complete* JSON value, carrying the unparsed remainder forward
//! until more bytes arrive.
//!
//! It is also defensive against a misbehaving child: a per-record byte cap stops
//! an unterminated or pathologically huge record (a tool result echoing a whole
//! file) from growing the buffer without bound — on overflow it resyncs to the
//! next newline and reports `overflowed()` so the runner can surface a note.

use serde_json::Value;

/// Accumulates stdout bytes and yields complete JSON values.
pub struct CliFramer {
    buf: Vec<u8>,
    /// Soft ceiling on a single in-progress record before we force a resync.
    cap: usize,
    /// Set when the last `push` had to drop bytes to recover from overflow.
    overflowed: bool,
}

impl CliFramer {
    /// `cap` is the max bytes a single incomplete record may occupy before the
    /// framer gives up on it and resyncs to the next newline.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap: cap.max(1024),
            overflowed: false,
        }
    }

    /// Feed a chunk of stdout; append any newly-complete JSON values to `out`.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<Value>) {
        // `overflowed` latches per call: true if *this* chunk had to drop bytes,
        // regardless of how many good records also parsed out of it.
        self.overflowed = false;
        self.buf.extend_from_slice(chunk);
        self.drain(out);
    }

    /// Whether the most recent [`push`](Self::push) dropped bytes to recover from
    /// a torn or oversized record. The runner uses this to emit a truncation note.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    fn drain(&mut self, out: &mut Vec<Value>) {
        loop {
            // Trim leading whitespace (the NDJSON separators, plus any \r\n).
            match self.buf.iter().position(|b| !b.is_ascii_whitespace()) {
                Some(0) => {}
                Some(start) => {
                    self.buf.drain(0..start);
                }
                None => {
                    // All whitespace (or empty) — nothing to parse.
                    self.buf.clear();
                    return;
                }
            }

            let mut iter =
                serde_json::Deserializer::from_slice(&self.buf).into_iter::<Value>();
            match iter.next() {
                Some(Ok(value)) => {
                    let consumed = iter.byte_offset();
                    drop(iter);
                    out.push(value);
                    self.buf.drain(0..consumed);
                    // Loop to pull any further complete values from the buffer.
                }
                Some(Err(e)) if e.is_eof() => {
                    drop(iter);
                    // Incomplete record: wait for more bytes, unless it has grown
                    // past the cap, in which case resync to the next newline.
                    if self.buf.len() > self.cap {
                        self.resync_to_newline();
                        continue;
                    }
                    return;
                }
                Some(Err(_)) => {
                    drop(iter);
                    // Malformed (not merely truncated): drop to the next newline
                    // and try to recover the following records.
                    self.resync_to_newline();
                }
                None => {
                    return;
                }
            }
        }
    }

    /// Drop everything up to and including the next `\n`, marking an overflow.
    /// If there is no newline yet, drop the whole buffer (it is unrecoverable
    /// junk at this point) so we never grow without bound.
    fn resync_to_newline(&mut self) {
        self.overflowed = true;
        match self.buf.iter().position(|&b| b == b'\n') {
            Some(nl) => {
                self.buf.drain(0..=nl);
            }
            None => {
                self.buf.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn collect(framer: &mut CliFramer, chunk: &[u8]) -> Vec<Value> {
        let mut out = Vec::new();
        framer.push(chunk, &mut out);
        out
    }

    #[test]
    fn two_ndjson_objects_one_chunk() {
        let mut f = CliFramer::new(64 * 1024);
        let got = collect(&mut f, b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(got, vec![json!({"a":1}), json!({"b":2})]);
    }

    #[test]
    fn object_split_across_chunks() {
        let mut f = CliFramer::new(64 * 1024);
        assert!(collect(&mut f, b"{\"a\":").is_empty());
        let got = collect(&mut f, b"123}\n");
        assert_eq!(got, vec![json!({"a":123})]);
    }

    #[test]
    fn newline_inside_string_value_is_not_a_separator() {
        let mut f = CliFramer::new(64 * 1024);
        // The string value contains a literal \n; must parse as ONE object.
        let got = collect(&mut f, b"{\"out\":\"line1\\nline2\"}\n");
        assert_eq!(got, vec![json!({"out":"line1\nline2"})]);
    }

    #[test]
    fn pretty_printed_object_with_internal_newlines() {
        let mut f = CliFramer::new(64 * 1024);
        let got = collect(&mut f, b"{\n  \"a\": 1,\n  \"b\": 2\n}\n");
        assert_eq!(got, vec![json!({"a":1,"b":2})]);
    }

    #[test]
    fn malformed_line_then_valid_recovers() {
        let mut f = CliFramer::new(64 * 1024);
        let got = collect(&mut f, b"not json at all\n{\"ok\":true}\n");
        assert_eq!(got, vec![json!({"ok":true})]);
        assert!(f.overflowed());
    }

    #[test]
    fn oversized_incomplete_record_resyncs() {
        let mut f = CliFramer::new(1024);
        let big = format!("{{\"x\":\"{}", "a".repeat(2048)); // unterminated, > cap
        assert!(collect(&mut f, big.as_bytes()).is_empty());
        assert!(f.overflowed());
        // After a newline + a good record, parsing resumes.
        let got = collect(&mut f, b"\n{\"y\":9}\n");
        assert_eq!(got, vec![json!({"y":9})]);
    }

    #[test]
    fn concatenated_without_newlines() {
        let mut f = CliFramer::new(64 * 1024);
        let got = collect(&mut f, b"{\"a\":1}{\"b\":2}");
        assert_eq!(got, vec![json!({"a":1}), json!({"b":2})]);
    }
}
