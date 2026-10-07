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
    secrets: Vec<SecretPattern>,
    pending: VecDeque<(u8, usize)>,
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
            secrets: secrets.into_iter().map(SecretPattern::new).collect(),
            pending: VecDeque::new(),
            line: Vec::new(),
            truncated: false,
        })
    }
    /// Masks before line truncation, retaining a suffix that might cross the next chunk.
    pub fn feed(&mut self, bytes: &[u8], end: bool, buffer: &mut LogBuffer) {
        for &byte in bytes {
            self.pending.push_back((byte, 0));
            let mut unresolved = 0;
            for secret in &mut self.secrets {
                if secret.advance(byte) && self.pending.len() >= secret.bytes.len() {
                    let start = self.pending.len() - secret.bytes.len();
                    if let Some((_, length)) = self.pending.get_mut(start) {
                        *length = (*length).max(secret.bytes.len());
                    }
                }
                unresolved = unresolved.max(secret.matched);
            }
            self.flush(unresolved, buffer);
        }
        if end {
            self.flush(0, buffer);
        }
        if end && (!self.line.is_empty() || self.truncated) {
            self.finish_line(buffer);
        }
    }
    fn flush(&mut self, unresolved: usize, buffer: &mut LogBuffer) {
        // Only prefixes that might complete in a later chunk remain undecided.
        while self.pending.len() > unresolved {
            if let Some((byte, length)) = self.pending.pop_front() {
                if length == 0 {
                    self.output(byte, buffer);
                } else {
                    for _ in 1..length {
                        self.pending.pop_front();
                    }
                    for &byte in b"********" {
                        self.output(byte, buffer);
                    }
                }
            }
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

// KMP keeps a failure table and cursor per secret instead of rescanning long prefixes.
// With at most 128 patterns, total matching work is O(128 * input bytes), even across chunks.
struct SecretPattern {
    bytes: Vec<u8>,
    fallback: Vec<usize>,
    matched: usize,
    #[cfg(test)]
    comparisons: usize,
}
impl SecretPattern {
    fn new(bytes: Vec<u8>) -> Self {
        let mut fallback = vec![0; bytes.len()];
        let mut matched = 0;
        for i in 1..bytes.len() {
            while matched > 0 && bytes[matched] != bytes[i] {
                matched = fallback[matched - 1];
            }
            if bytes[matched] == bytes[i] {
                matched += 1;
            }
            fallback[i] = matched;
        }
        Self {
            bytes,
            fallback,
            matched: 0,
            #[cfg(test)]
            comparisons: 0,
        }
    }
    fn advance(&mut self, byte: u8) -> bool {
        loop {
            #[cfg(test)]
            {
                self.comparisons += 1;
            }
            if self.bytes[self.matched] == byte {
                self.matched += 1;
                break;
            }
            if self.matched == 0 {
                return false;
            }
            self.matched = self.fallback[self.matched - 1];
        }
        if self.matched == self.bytes.len() {
            self.matched = self.fallback[self.matched - 1];
            true
        } else {
            false
        }
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
    fn long_common_prefixes_have_linear_comparison_work_and_bounded_pending_bytes() {
        let secrets = (0..128)
            .map(|i| format!("{}{:02x}", "a".repeat(LINE_BYTES - 2), i))
            .collect::<Vec<_>>();
        let mut decoder = LogDecoder::new(&secrets).unwrap();
        let mut buffer = LogBuffer::default();
        let raw = "a".repeat(4 * LINE_BYTES);
        for chunk in raw.as_bytes().chunks(8192) {
            decoder.feed(chunk, false, &mut buffer);
            assert!(decoder.pending.len() < LINE_BYTES);
        }
        decoder.feed(b"z\n", true, &mut buffer);
        assert_eq!(buffer.truncated_lines, 1);
        assert!(
            decoder
                .secrets
                .iter()
                .all(|s| s.comparisons <= 2 * (raw.len() + 2))
        );
    }
    #[test]
    fn leftmost_longest_matches_and_eof_prefixes_survive_byte_sized_chunks() {
        let secrets: Vec<String> = vec!["aba".into(), "ab".into(), "bab".into(), "aaaa".into()];
        for raw in ["ababa", "baba", "aaaaaa", "aab", "abaaba", "bababa", "a"] {
            let mut expected = String::new();
            let mut rest = raw;
            while !rest.is_empty() {
                if let Some(secret) = secrets
                    .iter()
                    .filter(|s| rest.starts_with(s.as_str()))
                    .max_by_key(|s| s.len())
                {
                    expected.push_str("********");
                    rest = &rest[secret.len()..];
                } else {
                    expected.push_str(&rest[..1]);
                    rest = &rest[1..];
                }
            }
            let mut decoder = LogDecoder::new(&secrets).unwrap();
            let mut buffer = LogBuffer::default();
            for byte in raw.bytes() {
                decoder.feed(&[byte], false, &mut buffer);
            }
            decoder.feed(&[], true, &mut buffer);
            assert_eq!(buffer.lines(), [expected], "{raw}");
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
