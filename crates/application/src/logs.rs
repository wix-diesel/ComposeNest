//! Memory-only, masked service logs with fixed display budgets.
use std::collections::VecDeque;

/// Maximum retained complete lines per subscription.
pub const LOG_LINES: usize = 2_000;
/// Maximum retained UTF-8 bytes, including line separators.
pub const LOG_BYTES: usize = 2 * 1024 * 1024;
/// Maximum UTF-8 bytes in one displayed line.
pub const LINE_BYTES: usize = 16 * 1024;

/// A bounded ring containing only already-masked text.
#[derive(Default)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    bytes: usize,
    /// Number of complete lines evicted by either display budget.
    pub dropped_lines: u64,
    /// Number of lines shortened to the per-line budget.
    pub truncated_lines: u64,
    /// Whether the CLI has finished or its subscription was cancelled.
    pub finished: bool,
    /// Whether the read failed; raw Docker diagnostics are never returned.
    pub failed: bool,
}
impl LogBuffer {
    /// Returns a bounded copy for a pull-based UI, without creating an output queue.
    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
    fn push(&mut self, bytes: &[u8], truncated: bool) {
        let mut line = String::from_utf8_lossy(bytes).into_owned();
        let mut end = line.len().min(LINE_BYTES);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        self.truncated_lines += u64::from(truncated || end < line.len());
        line.truncate(end);
        self.bytes += line.len() + 1;
        self.lines.push_back(line);
        while self.lines.len() > LOG_LINES || self.bytes > LOG_BYTES {
            if let Some(line) = self.lines.pop_front() {
                self.bytes -= line.len() + 1;
                self.dropped_lines += 1;
            }
        }
    }
}

/// One pipe's incremental masker and line decoder; stdout and stderr stay separate.
pub struct LogDecoder {
    secrets: Vec<Vec<u8>>,
    pending: Vec<u8>,
    line: Vec<u8>,
    truncated: bool,
}
impl LogDecoder {
    /// Rejects oversized masking inputs rather than ever displaying them unmasked.
    pub fn new(secrets: &[String]) -> Result<Self, &'static str> {
        if secrets.len() > 128 || secrets.iter().any(|s| s.len() > LINE_BYTES) {
            return Err("invalid_log_secrets");
        }
        let mut secrets: Vec<Vec<u8>> = secrets
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.as_bytes().to_vec())
            .collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Ok(Self {
            secrets,
            pending: Vec::new(),
            line: Vec::new(),
            truncated: false,
        })
    }
    /// Masks before line truncation, retaining a suffix that might cross the next chunk.
    pub fn feed(&mut self, bytes: &[u8], end: bool, buffer: &mut LogBuffer) {
        // Callers feed at most 8 KiB at once, so pending storage is bounded too.
        self.pending.extend_from_slice(bytes);
        let mut offset = 0;
        while offset < self.pending.len() {
            let rest = &self.pending[offset..];
            if !end
                && self
                    .secrets
                    .iter()
                    .any(|s| s.len() > rest.len() && s.starts_with(rest))
            {
                break;
            }
            if let Some(secret) = self
                .secrets
                .iter()
                .find(|s| self.pending[offset..].starts_with(s))
            {
                offset += secret.len();
                for byte in b"********" {
                    self.output(*byte, buffer);
                }
            } else {
                let byte = self.pending[offset];
                offset += 1;
                self.output(byte, buffer);
            }
        }
        self.pending.drain(..offset);
        if end && (!self.line.is_empty() || self.truncated) {
            self.finish_line(buffer);
        }
    }
    fn output(&mut self, byte: u8, buffer: &mut LogBuffer) {
        if byte == b'\n' {
            self.finish_line(buffer);
        } else if self.line.len() < LINE_BYTES {
            self.line.push(byte);
        } else {
            self.truncated = true;
        }
    }
    fn finish_line(&mut self, buffer: &mut LogBuffer) {
        buffer.push(&self.line, self.truncated);
        self.line.clear();
        self.truncated = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_chunk_boundary_is_masked_including_multiline_and_overlapping_values() {
        let secrets = vec!["abc".into(), "abcdef".into(), "秘密\npassword".into()];
        let raw = "before abcdef 秘密\npassword after\n".as_bytes();
        for split in 0..=raw.len() {
            let mut decoder = LogDecoder::new(&secrets).unwrap();
            let mut buffer = LogBuffer::default();
            decoder.feed(&raw[..split], false, &mut buffer);
            decoder.feed(&raw[split..], true, &mut buffer);
            assert_eq!(buffer.lines(), ["before ******** ******** after"]);
        }
    }
    #[test]
    fn both_ring_budgets_and_utf8_line_limit_count_loss() {
        let mut decoder = LogDecoder::new(&[]).unwrap();
        let mut buffer = LogBuffer::default();
        for _ in 0..2_100 {
            decoder.feed(b"line\n", false, &mut buffer);
        }
        assert_eq!(buffer.lines().len(), LOG_LINES);
        assert_eq!(buffer.dropped_lines, 100);
        let long = "界".repeat(8_000);
        for _ in 0..200 {
            for chunk in long.as_bytes().chunks(8192) {
                decoder.feed(chunk, false, &mut buffer);
            }
            decoder.feed(b"\n", false, &mut buffer);
        }
        assert!(buffer.bytes <= LOG_BYTES);
        assert_eq!(buffer.truncated_lines, 200);
        assert!(buffer.lines().iter().all(|s| s.len() <= LINE_BYTES));
        assert!(buffer.dropped_lines > 100);
    }
    #[test]
    fn masking_precedes_long_line_discard_and_each_pipe_has_its_own_tail() {
        let mut buffer = LogBuffer::default();
        let mut out = LogDecoder::new(&["secret".into()]).unwrap();
        let mut err = LogDecoder::new(&["secret".into()]).unwrap();
        out.feed(b"sec", false, &mut buffer);
        err.feed(b"secret\n", true, &mut buffer);
        out.feed(b"ret\n", true, &mut buffer);
        assert_eq!(buffer.lines(), ["********", "********"]);
        assert!(LogDecoder::new(&["x".repeat(LINE_BYTES + 1)]).is_err());
    }
}
