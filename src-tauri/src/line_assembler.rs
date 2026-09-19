//! Line reassembly for the RTT byte stream, split out so it can be unit-tested
//! without a J-Link probe or a Tauri channel.
//!
//! RTT delivers an unstructured byte stream: one "line" may arrive across many
//! reads, and a single read may contain many lines. Each emitted line carries
//! the arrival time of its **first** byte, which is what makes the timestamp
//! meaningful even when the rest of the line trickles in later.

use std::time::{Instant, SystemTime};

/// A completed line: raw bytes (including its terminator) plus the arrival time
/// of its first byte.
#[derive(Debug, Clone)]
pub struct AssembledLine {
    pub raw: Vec<u8>,
    pub first_byte_at: Instant,
    pub first_byte_wall: SystemTime,
}

#[derive(Default)]
pub struct LineAssembler {
    buf: Vec<u8>,
    first: Option<(Instant, SystemTime)>,
}

impl LineAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Feed newly received bytes; returns every line completed by this chunk.
    pub fn feed(&mut self, data: &[u8], now: Instant, wall: SystemTime) -> Vec<AssembledLine> {
        let mut out = Vec::new();
        if data.is_empty() {
            return out;
        }
        let mut cursor = 0usize;
        while cursor < data.len() {
            if self.first.is_none() {
                self.first = Some((now, wall));
            }
            match data[cursor..].iter().position(|&b| b == b'\n') {
                Some(rel) => {
                    let end = cursor + rel + 1; // include the '\n'
                    self.buf.extend_from_slice(&data[cursor..end]);
                    let (first_byte_at, first_byte_wall) = self.first.take().unwrap();
                    out.push(AssembledLine {
                        raw: std::mem::take(&mut self.buf),
                        first_byte_at,
                        first_byte_wall,
                    });
                    cursor = end;
                }
                None => {
                    self.buf.extend_from_slice(&data[cursor..]);
                    break;
                }
            }
        }
        out
    }

    /// Take the incomplete trailing line, if any (e.g. a prompt with no newline).
    pub fn take_partial(&mut self) -> Option<AssembledLine> {
        if self.buf.is_empty() {
            return None;
        }
        let (first_byte_at, first_byte_wall) = self.first.take()?;
        Some(AssembledLine {
            raw: std::mem::take(&mut self.buf),
            first_byte_at,
            first_byte_wall,
        })
    }
}

/// Decode a raw line, dropping the trailing CR/LF and replacing invalid UTF-8
/// (a multi-byte character split across reads degrades to the replacement char).
pub fn decode_line(raw: &[u8]) -> String {
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r') {
        end -= 1;
    }
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn single_complete_line() {
        let mut a = LineAssembler::new();
        let lines = a.feed(b"hello\n", t0(), SystemTime::now());
        assert_eq!(lines.len(), 1);
        assert_eq!(decode_line(&lines[0].raw), "hello");
        assert!(a.is_empty());
    }

    #[test]
    fn multiple_lines_in_one_chunk() {
        let mut a = LineAssembler::new();
        let lines = a.feed(b"a\nbb\nccc\n", t0(), SystemTime::now());
        let texts: Vec<_> = lines.iter().map(|l| decode_line(&l.raw)).collect();
        assert_eq!(texts, vec!["a", "bb", "ccc"]);
        assert!(a.is_empty());
    }

    #[test]
    fn line_split_across_reads() {
        let mut a = LineAssembler::new();
        let first = t0();
        assert!(a.feed(b"par", first, SystemTime::now()).is_empty());
        assert!(!a.is_empty());
        let later = first + Duration::from_millis(500);
        let lines = a.feed(b"tial\n", later, SystemTime::now());
        assert_eq!(lines.len(), 1);
        assert_eq!(decode_line(&lines[0].raw), "partial");
        // Timestamp must be the FIRST byte's arrival, not the last chunk's.
        assert_eq!(lines[0].first_byte_at, first);
    }

    #[test]
    fn crlf_is_trimmed() {
        let mut a = LineAssembler::new();
        let lines = a.feed(b"line\r\n", t0(), SystemTime::now());
        assert_eq!(decode_line(&lines[0].raw), "line");
    }

    #[test]
    fn blank_line_is_preserved() {
        let mut a = LineAssembler::new();
        let lines = a.feed(b"\n", t0(), SystemTime::now());
        assert_eq!(lines.len(), 1);
        assert_eq!(decode_line(&lines[0].raw), "");
    }

    #[test]
    fn partial_line_flushed_on_idle() {
        let mut a = LineAssembler::new();
        let first = t0();
        a.feed(b"no-newline-prompt> ", first, SystemTime::now());
        assert!(!a.is_empty());
        let partial = a.take_partial().expect("partial line expected");
        assert_eq!(decode_line(&partial.raw), "no-newline-prompt> ");
        assert_eq!(partial.first_byte_at, first);
        assert!(a.is_empty());
        assert!(a.take_partial().is_none());
    }

    #[test]
    fn new_line_after_partial_gets_own_timestamp() {
        let mut a = LineAssembler::new();
        let first = t0();
        // Trailing partial from an earlier read, then a full line in a later read.
        a.feed(b"abc", first, SystemTime::now());
        let later = first + Duration::from_secs(1);
        let lines = a.feed(b"def\n", later, SystemTime::now());
        assert_eq!(lines.len(), 1);
        assert_eq!(decode_line(&lines[0].raw), "abcdef");
        // The line started with 'a' back at `first`.
        assert_eq!(lines[0].first_byte_at, first);
    }

    #[test]
    fn consecutive_lines_after_partial_split_at_newline() {
        let mut a = LineAssembler::new();
        let first = t0();
        a.feed(b"aa", first, SystemTime::now());
        let later = first + Duration::from_secs(1);
        let lines = a.feed(b"a\nbbb\n", later, SystemTime::now());
        assert_eq!(lines.len(), 2);
        assert_eq!(decode_line(&lines[0].raw), "aaa");
        assert_eq!(decode_line(&lines[1].raw), "bbb");
        // First line began at `first`; second line began when its 'b' arrived.
        assert_eq!(lines[0].first_byte_at, first);
        assert_eq!(lines[1].first_byte_at, later);
    }

    #[test]
    fn invalid_utf8_is_replaced_not_dropped() {
        let mut a = LineAssembler::new();
        // 0xFF is never valid UTF-8; the line must still decode to something
        // and must not lose the surrounding ASCII.
        let lines = a.feed(b"ok\xFFbad\n", t0(), SystemTime::now());
        let text = decode_line(&lines[0].raw);
        assert!(text.starts_with("ok"));
        assert!(text.ends_with("bad"));
    }

    #[test]
    fn utf8_split_across_reads_degrades_gracefully() {
        let mut a = LineAssembler::new();
        // "中" is E4 B8 AD. Split after the first byte.
        let bytes = "中\n".as_bytes();
        assert!(a.feed(&bytes[..1], t0(), SystemTime::now()).is_empty());
        let lines = a.feed(&bytes[1..], t0() + Duration::from_millis(10), SystemTime::now());
        assert_eq!(lines.len(), 1);
        // Reassembly operates on bytes, so the character survives intact here.
        assert_eq!(decode_line(&lines[0].raw), "中");
    }

    /// Capturing several RTT up channels must not splice them together. Each
    /// channel gets its own assembler; this pins down why that is necessary.
    #[test]
    fn separate_assemblers_keep_channels_independent() {
        let mut ch0 = LineAssembler::new();
        let mut ch1 = LineAssembler::new();
        let t = t0();

        // Interleave partial writes from two channels, as polling would.
        assert!(ch0.feed(b"TEMP", t, SystemTime::now()).is_empty());
        assert!(ch1.feed(b"CMD", t, SystemTime::now()).is_empty());
        let l0 = ch0.feed(b"=25.6\n", t, SystemTime::now());
        let l1 = ch1.feed(b"_OK\n", t, SystemTime::now());

        assert_eq!(l0.len(), 1);
        assert_eq!(l1.len(), 1);
        assert_eq!(decode_line(&l0[0].raw), "TEMP=25.6");
        assert_eq!(decode_line(&l1[0].raw), "CMD_OK");
    }

    /// The failure mode this avoids: one shared assembler would merge the two
    /// channels' partial lines into a single bogus line.
    #[test]
    fn shared_assembler_would_splice_channels() {
        let mut shared = LineAssembler::new();
        let t = t0();
        shared.feed(b"TEMP", t, SystemTime::now()); // channel 0
        shared.feed(b"CMD", t, SystemTime::now()); // channel 1, wrong
        let lines = shared.feed(b"=25.6\n", t, SystemTime::now());
        assert_eq!(lines.len(), 1);
        // Channel 1's bytes leaked into channel 0's line.
        assert_eq!(decode_line(&lines[0].raw), "TEMPCMD=25.6");
    }
}
