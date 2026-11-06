//! WebSocket framing (RFC 6455), both ends: the server side for a phone on
//! the same Wi-Fi, the client side for the relay. Text frames only, plus
//! ping, pong and close.

use std::io::{self, Read, Write};

use crate::hash::{base64, random_bytes, sha1};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The `Sec-WebSocket-Accept` answer to a client's key.
pub fn accept(key: &str) -> String {
    base64(&sha1(format!("{}{GUID}", key.trim()).as_bytes()))
}

/// One whole message.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// Collects bytes as they arrive and hands back whole frames.
#[derive(Default)]
pub struct Reader {
    buffer: Vec<u8>,
    /// The pieces of a text message sent in more than one frame.
    partial: Vec<u8>,
}

/// The longest message taken: far more than a paste from a phone.
const LONGEST: usize = 1 << 20;

impl Reader {
    /// Bytes that arrived some other way (after an HTTP response head).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Reads what the stream has (it may time out, which is not an error
    /// here) and returns every frame now complete. `Err` when the
    /// connection is over or broken.
    pub fn read(&mut self, stream: &mut impl Read) -> io::Result<Vec<Frame>> {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed")),
            Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
        let mut frames = Vec::new();
        while let Some((frame, used)) = self.parse()? {
            self.buffer.drain(..used);
            if let Some(frame) = frame {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    /// One frame from the front of the buffer, and how many bytes it took.
    fn parse(&mut self) -> io::Result<Option<(Option<Frame>, usize)>> {
        let b = &self.buffer;
        if b.len() < 2 {
            return Ok(None);
        }
        let fin = b[0] & 0x80 != 0;
        let opcode = b[0] & 0x0F;
        let masked = b[1] & 0x80 != 0;
        let mut len = (b[1] & 0x7F) as usize;
        let mut at = 2;
        if len == 126 {
            if b.len() < 4 {
                return Ok(None);
            }
            len = u16::from_be_bytes([b[2], b[3]]) as usize;
            at = 4;
        } else if len == 127 {
            if b.len() < 10 {
                return Ok(None);
            }
            let long = u64::from_be_bytes(b[2..10].try_into().unwrap_or_default());
            len = usize::try_from(long).unwrap_or(usize::MAX);
            at = 10;
        }
        if len > LONGEST {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too long"));
        }
        let mask_at = at;
        if masked {
            at += 4;
        }
        if b.len() < at + len {
            return Ok(None);
        }
        let mut payload = b[at..at + len].to_vec();
        if masked {
            let mask = [b[mask_at], b[mask_at + 1], b[mask_at + 2], b[mask_at + 3]];
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }
        let used = at + len;
        let frame = match opcode {
            0x0 | 0x1 => {
                self.partial.extend_from_slice(&payload);
                if self.partial.len() > LONGEST {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "too long"));
                }
                if fin {
                    let whole = std::mem::take(&mut self.partial);
                    Some(Frame::Text(String::from_utf8(whole).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))?))
                } else {
                    None
                }
            }
            0x8 => Some(Frame::Close),
            0x9 => Some(Frame::Ping(payload)),
            0xA => Some(Frame::Pong),
            _ => None,
        };
        Ok(Some((frame, used)))
    }
}

/// A frame's bytes. A client must mask what it sends; a server must not.
pub fn encode(opcode: u8, payload: &[u8], mask: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    let m = if mask { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(m | n as u8),
        n if n < 65536 => {
            out.push(m | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(m | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    if mask {
        let key = random_bytes(4);
        out.extend_from_slice(&key);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
    } else {
        out.extend_from_slice(payload);
    }
    out
}

pub fn send_text(stream: &mut impl Write, text: &str, mask: bool) -> io::Result<()> {
    stream.write_all(&encode(0x1, text.as_bytes(), mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_masked_text_frame_from_rfc_6455_reads_as_hello() {
        let bytes = [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
        let mut reader = Reader::default();
        assert_eq!(reader.read(&mut &bytes[..]).unwrap(), [Frame::Text("Hello".into())]);
    }

    #[test]
    fn frames_survive_arriving_in_pieces_and_round_trip() {
        let text = "x".repeat(70_000) + " \u{e9}";
        let bytes = [encode(0x1, b"one", true), encode(0x1, text.as_bytes(), false), encode(0x9, b"p", true)].concat();
        let mut reader = Reader::default();
        let mut got = Vec::new();
        for piece in bytes.chunks(1000) {
            got.extend(reader.read(&mut &piece[..]).unwrap());
        }
        assert_eq!(got, [Frame::Text("one".into()), Frame::Text(text), Frame::Ping(b"p".to_vec())]);
    }
}
