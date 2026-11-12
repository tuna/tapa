//! The PLIO text files the AI Engine simulators read and write.
//!
//! Every TAPA PLIO carries 32-bit words, so a file holds one beat per line
//! as decimal 32-bit integers. A `TLAST` line marks the beat on the line
//! after it, in the files the simulators write as in those they read: an
//! output file interleaves `T <time> <unit>` timestamp lines and `TLAST`
//! markers with the data lines, and every input beat follows a `TLAST`
//! line, as every beat into the AI Engine carries TLAST on hardware (the
//! interface holds a lone 32-bit word without it). The live relay writes and
//! reads both as the data arrives, so each side keeps the partial beat or
//! line between calls.

/// Turns a stream of words into input beat lines.
pub struct BeatWriter {
    beat_words: usize,
    transfer_words: usize,
    pending: Vec<u32>,
    pub words: u64,
}

impl BeatWriter {
    pub fn new(beat_words: u32, transfer_words: u32) -> Self {
        let beat_words = (beat_words as usize).max(1);
        Self {
            beat_words,
            transfer_words: (transfer_words as usize).max(beat_words),
            pending: Vec::new(),
            words: 0,
        }
    }

    /// The lines of every beat `words` completes.
    pub fn push(&mut self, words: &[u32]) -> String {
        self.words += words.len() as u64;
        self.pending.extend_from_slice(words);
        let whole = self.pending.len() / self.beat_words * self.beat_words;
        let out = render(&self.pending[..whole], self.beat_words);
        self.pending.drain(..whole);
        out
    }

    /// The rest, zero-padded to a whole transfer (a whole beat and a whole
    /// AI Engine frame), once the input has ended.
    pub fn finish(&mut self) -> String {
        let total = usize::try_from(self.words).unwrap_or(usize::MAX);
        let padded = total.next_multiple_of(self.transfer_words);
        self.pending
            .resize(self.pending.len() + (padded - total), 0);
        let out = render(&self.pending, self.beat_words);
        self.pending.clear();
        out
    }
}

fn render(words: &[u32], beat_words: usize) -> String {
    let mut out = String::new();
    for beat in words.chunks(beat_words) {
        let line: Vec<String> = beat.iter().map(|w| w.cast_signed().to_string()).collect();
        out.push_str("TLAST\n");
        out.push_str(&line.join(" "));
        out.push('\n');
    }
    out
}

/// Parses output text as it is appended.
#[derive(Default)]
pub struct LineParser {
    partial: Vec<u8>,
    /// The time of each beat in nanoseconds; only `aiesimulator` stamps them.
    pub beat_times_ns: Vec<f64>,
}

impl LineParser {
    /// Appends the words of every line `bytes` completes to `words`.
    pub fn feed(&mut self, bytes: &[u8], words: &mut Vec<u32>) {
        self.partial.extend_from_slice(bytes);
        let Some(last_newline) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let complete: Vec<u8> = self.partial.drain(..=last_newline).collect();
        for line in String::from_utf8_lossy(&complete).lines() {
            self.line(line, words);
        }
    }

    fn line(&mut self, line: &str, words: &mut Vec<u32>) {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("T") => {
                let value: f64 = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let scale = match fields.next() {
                    Some("ps") => 1e-3,
                    Some("us") => 1e3,
                    _ => 1.0,
                };
                self.beat_times_ns.push(value * scale);
            }
            Some("TLAST") | None => {}
            Some(first) => {
                for word in std::iter::once(first).chain(fields) {
                    let value = word
                        .parse::<i32>()
                        .map(i32::cast_unsigned)
                        .or_else(|_| word.parse::<u32>());
                    if let Ok(value) = value {
                        words.push(value);
                    }
                }
            }
        }
    }
}

/// Where a replayed output first departs from what software simulation
/// pushed on it: the same words, then only the zero padding that rounds
/// the last transfer up, are a match.
pub fn first_mismatch(got: &[u32], want: &[u32], transfer_words: usize) -> Option<usize> {
    if let Some(index) = got.iter().zip(want).position(|(got, want)| got != want) {
        return Some(index);
    }
    let padded = want.len().next_multiple_of(transfer_words.max(1));
    if got.len() < want.len() {
        Some(got.len())
    } else if got.len() > padded {
        Some(padded)
    } else {
        got[want.len()..]
            .iter()
            .position(|word| *word != 0)
            .map(|index| want.len() + index)
    }
}

/// `, beats at <first>..<last> ns (<mean> ns apart)`, or nothing when no
/// beat was timed.
pub fn timing(beat_times_ns: &[f64]) -> String {
    let (Some(first), Some(last)) = (beat_times_ns.first(), beat_times_ns.last()) else {
        return String::new();
    };
    // A lone beat has no interval to average.
    let gaps = u32::try_from(beat_times_ns.len() - 1).unwrap_or(u32::MAX);
    let spacing = if gaps > 0 {
        format!(" ({:.2} ns apart)", (last - first) / f64::from(gaps))
    } else {
        String::new()
    };
    format!(", beats at {first:.1}..{last:.1} ns{spacing}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_beats_pad_to_a_whole_transfer_and_output_timestamps_scale() {
        let mut writer = BeatWriter::new(4, 4);
        assert_eq!(writer.push(&[1, 2, 3]), "");
        assert_eq!(writer.push(&[0xffff_ffff, 5]), "TLAST\n1 2 3 -1\n");
        assert_eq!(writer.finish(), "TLAST\n5 0 0 0\n");
        let mut frames = BeatWriter::new(2, 4);
        assert_eq!(frames.push(&[1, 2]), "TLAST\n1 2\n");
        assert_eq!(frames.finish(), "TLAST\n0 0\n", "a partial frame is padded");
        // aiesimulator applies a `TLAST` line to the beat after it: a lone
        // word with its `TLAST` after it waited forever in the interface.
        assert_eq!(BeatWriter::new(1, 1).push(&[63]), "TLAST\n63\n");

        let mut parser = LineParser::default();
        let mut words = Vec::new();
        parser.feed(b"T 6956800 ps\n0 1 2", &mut words);
        assert_eq!(words, [] as [u32; 0], "a partial line waits");
        parser.feed(b" 3\nT 6960 ns\nTLAST\n-1 5 6 7\n", &mut words);
        assert_eq!(words, [0, 1, 2, 3, 0xffff_ffff, 5, 6, 7]);
        assert_eq!(parser.beat_times_ns, [6956.8, 6960.0]);
    }

    #[test]
    fn a_replayed_output_matches_up_to_zero_padding_of_its_last_transfer() {
        assert_eq!(first_mismatch(&[1, 2], &[1, 2], 1), None);
        assert_eq!(first_mismatch(&[1], &[1, 2], 1), Some(1), "short");
        assert_eq!(
            first_mismatch(&[1, 2, 1, 2], &[1, 2], 1),
            Some(2),
            "surplus"
        );
        assert_eq!(first_mismatch(&[9], &[], 1), Some(0));
        assert_eq!(first_mismatch(&[1, 2, 3, 0], &[1, 2, 3], 4), None);
        assert_eq!(
            first_mismatch(&[1, 2, 3, 4], &[1, 2, 3], 4),
            Some(3),
            "padding is zero"
        );
        assert_eq!(
            first_mismatch(&[1, 2, 0, 0, 0], &[1, 2], 4),
            Some(4),
            "one transfer only"
        );
        let padded = [1, 2, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            first_mismatch(&padded, &[1, 2], 12),
            None,
            "a partial last beat"
        );
        let mut nonzero = padded;
        nonzero[3] = 9;
        assert_eq!(first_mismatch(&nonzero, &[1, 2], 12), Some(3));
        assert_eq!(first_mismatch(&[1, 3], &[1, 2], 1), Some(1), "differs");
    }
}
