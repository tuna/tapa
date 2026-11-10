//! The host's control-register requests during a simulated run.
//!
//! A kernel the host moves frames with as it runs (a ring) takes them: the
//! testbench polls a request queue, performs each request on the control
//! slave, and answers on a reply queue. Once the kernel is done it says so
//! on the reply queue and keeps serving until the host asks it to finish.
//!
//! The layouts are the testbench's too: a request is three little-endian
//! words, `{op, offset, value}` (op 0 reads, 1 writes, 2 finishes); a reply
//! is two, `{kind, value}` (kind 0 answers the request before it, kind 1
//! says the kernel is done).

/// Bytes of a request.
pub const REQUEST_BYTES: usize = 12;
/// Bytes of a reply.
pub const REPLY_BYTES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Read { offset: u32 },
    Write { offset: u32, value: u32 },
    Finish,
}

impl Request {
    #[must_use]
    pub fn encode(self) -> [u8; REQUEST_BYTES] {
        let (op, offset, value) = match self {
            Self::Read { offset } => (0u32, offset, 0),
            Self::Write { offset, value } => (1, offset, value),
            Self::Finish => (2, 0, 0),
        };
        let words: [u32; 3] = (op, offset, value).into();
        let mut out = [0; REQUEST_BYTES];
        for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *bytes = word.to_le_bytes();
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// The value a read returned; 0 for a write.
    Value(u32),
    Done,
}

impl Reply {
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        match word(0)? {
            0 => Some(Self::Value(word(4)?)),
            1 => Some(Self::Done),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_is_three_little_endian_words() {
        let bytes = Request::Write {
            offset: 0x30,
            value: 0x0102_0304,
        }
        .encode();
        assert_eq!(bytes, [1, 0, 0, 0, 0x30, 0, 0, 0, 4, 3, 2, 1]);
    }

    #[test]
    fn replies_carry_a_value_or_the_end_of_the_run() {
        assert_eq!(
            Reply::decode(&[0, 0, 0, 0, 7, 0, 0, 0]),
            Some(Reply::Value(7))
        );
        assert_eq!(Reply::decode(&[1, 0, 0, 0, 0, 0, 0, 0]), Some(Reply::Done));
        assert_eq!(Reply::decode(&[2, 0, 0, 0, 0, 0, 0, 0]), None);
    }
}
