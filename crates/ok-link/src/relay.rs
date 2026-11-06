//! The computer's side of the relay: one WebSocket out to it, kept open, and
//! every message through it sealed with AES-256-GCM. The relay sees a room
//! name and sealed bytes; it cannot read or forge a word.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::gcm::{open, seal};
use crate::hash::{base64, base64_decode, base64url, random_bytes};
use crate::ws::{self, Frame, Reader};
use crate::{room_and_key, said, Said, Shared};

pub(crate) fn start(shared: Arc<Shared>, desk: String, secret: [u8; 32]) {
    let _ = std::thread::Builder::new().name("ok-link-relay".into()).spawn(move || {
        let mut pause = 1u64;
        while !shared.stop.load(Ordering::Relaxed) {
            let began = Instant::now();
            let _ = run(&shared, &desk, &secret);
            shared.relay_up.store(false, Ordering::Relaxed);
            shared.relay_phones.store(0, Ordering::Relaxed);
            shared.wake();
            // A connection that lasted a while starts the waiting over.
            if began.elapsed() > Duration::from_secs(60) {
                pause = 1;
            }
            for _ in 0..pause * 10 {
                if shared.stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            pause = (pause * 2).min(30);
        }
    });
}

/// Seals a message for the phone: base64 of the nonce and the sealed bytes.
pub(crate) fn seal_for_phone(key: &[u8; 32], message: &str) -> String {
    let nonce: [u8; 12] = random_bytes(12).try_into().unwrap_or([0; 12]);
    let sealed = seal(key, &nonce, b"d", message.as_bytes());
    base64url(&[nonce.as_slice(), &sealed].concat())
}

/// Opens a message from the phone, or `None` if it is not one.
pub(crate) fn open_from_phone(key: &[u8; 32], text: &str) -> Option<String> {
    let bytes = base64_decode(text)?;
    if bytes.len() < 12 + 16 {
        return None;
    }
    let nonce: [u8; 12] = bytes[..12].try_into().ok()?;
    String::from_utf8(open(key, &nonce, b"p", &bytes[12..])?).ok()
}

fn run(shared: &Shared, desk: &str, secret: &[u8; 32]) -> io::Result<()> {
    let (room, key) = room_and_key(secret);
    let address = if desk.contains(':') { desk.to_string() } else { format!("{desk}:80") };
    let host = desk.split(':').next().unwrap_or(desk);
    let target = address.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))?;
    let mut stream = TcpStream::connect_timeout(&target, Duration::from_secs(8))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(8)))?;
    let challenge = base64(&random_bytes(16));
    let request = format!("GET /r/{room}/desk HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {challenge}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: ok\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut buffer = Vec::new();
    let end = loop {
        let mut chunk = [0u8; 2048];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed"));
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > 16 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
    };
    let head = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
    let expected = ws::accept(&challenge).to_ascii_lowercase();
    if !head.starts_with("http/1.1 101") || !head.contains(&expected) {
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "the relay said no"));
    }
    let mut reader = Reader::default();
    reader.feed(&buffer[end + 4..]);
    stream.set_read_timeout(Some(Duration::from_millis(40)))?;
    shared.relay_up.store(true, Ordering::Relaxed);
    shared.wake();

    let mut sent = u64::MAX;
    let mut heard = Instant::now();
    let mut pinged = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        for frame in reader.read(&mut stream)? {
            heard = Instant::now();
            match frame {
                // Plain words from the relay itself: how many phones are in.
                Frame::Text(text) if text.starts_with('!') => {
                    if let Some(n) = text.strip_prefix("!phones=").and_then(|n| n.parse::<usize>().ok()) {
                        let before = shared.relay_phones.swap(n, Ordering::Relaxed);
                        if n > before {
                            sent = u64::MAX;
                        }
                        shared.wake();
                    }
                }
                Frame::Text(text) => match open_from_phone(&key, &text).map(|m| said(&m)) {
                    Some(Said::Op(op)) => shared.push(op),
                    Some(Said::Hello) => sent = u64::MAX,
                    _ => {}
                },
                Frame::Ping(payload) => stream.write_all(&ws::encode(0xA, &payload, true))?,
                Frame::Close => return Ok(()),
                Frame::Pong => {}
            }
        }
        let (version, json) = shared.status();
        if version != sent && !json.is_empty() {
            ws::send_text(&mut stream, &seal_for_phone(&key, &format!("s{json}")), true)?;
            sent = version;
        }
        if pinged.elapsed() > Duration::from_secs(20) {
            stream.write_all(&ws::encode(0x9, b"ok", true))?;
            pinged = Instant::now();
        }
        if heard.elapsed() > Duration::from_secs(70) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the relay went quiet"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_messages_open_only_the_right_way() {
        let key = [9u8; 32];
        let sealed = seal_for_phone(&key, "sHello");
        assert_eq!(open_from_phone(&key, &sealed), None, "a message to the phone is not taken as one from it");
        let nonce = [1u8; 12];
        let from_phone = base64url(&[nonce.as_slice(), &seal(&key, &nonce, b"p", "tWords".as_bytes())].concat());
        assert_eq!(open_from_phone(&key, &from_phone).as_deref(), Some("tWords"));
        assert_eq!(open_from_phone(&[8u8; 32], &from_phone), None);
    }
}
