//! Length-prefixed framing. Each message is a 4-byte big-endian length followed
//! by that many payload bytes. The reader accepts data in any chunking, so a
//! message split across several socket reads, or several messages arriving in
//! one read, both decode correctly.

/// Largest payload accepted in one frame. Anything bigger is treated as a
/// broken or hostile peer rather than read into memory.
pub const MAX_FRAME: usize = 64 * 1024;

const HEADER: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    TooLarge(usize),
}

/// Wraps `payload` in a frame ready to send.
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Collects raw bytes from a socket and yields complete payloads.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Returns the next complete payload, or `None` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.buf.len() < HEADER {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME {
            return Err(FrameError::TooLarge(len));
        }
        if self.buf.len() < HEADER + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER..HEADER + len].to_vec();
        self.buf.drain(..HEADER + len);
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_frame_round_trips() {
        let mut r = FrameReader::default();
        r.push(&encode(b"ping").unwrap());
        assert_eq!(r.next_frame(), Ok(Some(b"ping".to_vec())));
        assert_eq!(r.next_frame(), Ok(None));
    }

    #[test]
    fn frame_split_across_pushes_waits_for_the_rest() {
        let bytes = encode(b"split message").unwrap();
        let mut r = FrameReader::default();
        r.push(&bytes[..3]);
        assert_eq!(r.next_frame(), Ok(None));
        r.push(&bytes[3..10]);
        assert_eq!(r.next_frame(), Ok(None));
        r.push(&bytes[10..]);
        assert_eq!(r.next_frame(), Ok(Some(b"split message".to_vec())));
    }

    #[test]
    fn several_frames_in_one_push_come_out_in_order() {
        let mut bytes = encode(b"one").unwrap();
        bytes.extend(encode(b"two").unwrap());
        bytes.extend(encode(b"").unwrap());
        let mut r = FrameReader::default();
        r.push(&bytes);
        assert_eq!(r.next_frame(), Ok(Some(b"one".to_vec())));
        assert_eq!(r.next_frame(), Ok(Some(b"two".to_vec())));
        assert_eq!(r.next_frame(), Ok(Some(Vec::new())));
        assert_eq!(r.next_frame(), Ok(None));
    }

    #[test]
    fn oversized_frames_are_rejected() {
        assert_eq!(
            encode(&vec![0u8; MAX_FRAME + 1]),
            Err(FrameError::TooLarge(MAX_FRAME + 1))
        );
        let mut r = FrameReader::default();
        r.push(&((MAX_FRAME as u32 + 1).to_be_bytes()));
        assert_eq!(r.next_frame(), Err(FrameError::TooLarge(MAX_FRAME + 1)));
    }
}
