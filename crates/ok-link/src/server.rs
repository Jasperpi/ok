//! The phone page served from this computer, for a phone on the same Wi-Fi:
//! a few fixed files over HTTP and one WebSocket, all behind the token.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::ws::{self, Frame, Reader};
use crate::{said, Said, Shared, PORT};

pub(crate) const PAGE: &str = include_str!("../page/phone.html");

/// Gallant as a web font, made once.
pub(crate) fn font() -> &'static [u8] {
    static FONT: OnceLock<Vec<u8>> = OnceLock::new();
    FONT.get_or_init(|| crate::ttf::build(ok_gfx::gallant(), "ok Gallant"))
}

/// Starts listening on the first free port from [`PORT`]; `None` if none is.
pub(crate) fn start(shared: Arc<Shared>, icon_png: Vec<u8>, relay_page: Option<String>) -> Option<u16> {
    let (listener, port) = (PORT..PORT + 10).find_map(|port| TcpListener::bind(("0.0.0.0", port)).ok().map(|l| (l, port)))?;
    listener.set_nonblocking(true).ok()?;
    let icon = Arc::new(icon_png);
    let relay = Arc::new(relay_page.unwrap_or_default());
    std::thread::Builder::new()
        .name("ok-link-server".into())
        .spawn(move || {
            while !shared.stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (shared, icon, relay) = (shared.clone(), icon.clone(), relay.clone());
                        let _ = std::thread::Builder::new().name("ok-link-phone".into()).spawn(move || {
                            let _ = serve(stream, &shared, &icon, &relay);
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => std::thread::sleep(Duration::from_millis(500)),
                }
            }
        })
        .ok()?;
    Some(port)
}

/// A request's line and headers.
struct Request {
    path: String,
    query: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|pair| pair.strip_prefix(name).and_then(|rest| rest.strip_prefix('=')))
    }
}

/// Reads the head of a request; whatever came after it is returned too.
fn read_head(stream: &mut TcpStream) -> io::Result<(Request, Vec<u8>)> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
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
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let rest = buffer[end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let target = first.split(' ').nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines.filter_map(|line| line.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Ok((Request { path: path.to_string(), query: query.to_string(), headers }, rest))
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, cache: &str, body: &[u8]) -> io::Result<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn serve(mut stream: TcpStream, shared: &Shared, icon: &[u8], relay: &str) -> io::Result<()> {
    let (request, rest) = read_head(&mut stream)?;
    let allowed = request.param("t") == Some(shared.token.as_str());
    match request.path.as_str() {
        "/" | "/index.html" if allowed => {
            let page = PAGE.replace("{{RELAY}}", relay);
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", "no-store", page.as_bytes())
        }
        "/ws" if allowed && request.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket")) => {
            let Some(key) = request.header("sec-websocket-key") else { return Ok(()) };
            let answer = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept(key));
            stream.write_all(answer.as_bytes())?;
            session(stream, shared, rest)
        }
        "/manifest.webmanifest" if allowed => {
            let manifest = format!(
                "{{\"name\":\"ok\",\"short_name\":\"ok\",\"start_url\":\"/?t={}\",\"display\":\"standalone\",\"background_color\":\"#35251A\",\"theme_color\":\"#35251A\",\"icons\":[{{\"src\":\"/icon.png\",\"sizes\":\"192x192\",\"type\":\"image/png\"}}]}}",
                shared.token
            );
            respond(&mut stream, "200 OK", "application/manifest+json", "no-store", manifest.as_bytes())
        }
        "/gallant.ttf" => respond(&mut stream, "200 OK", "font/ttf", "max-age=86400", font()),
        "/icon.png" => respond(&mut stream, "200 OK", "image/png", "max-age=86400", icon),
        _ => {
            let body = "<!doctype html><meta name=viewport content='width=device-width'><title>ok</title><p style='font:18px monospace;padding:16px'>This address has changed. Scan the code on the computer again: Esc, then phone.</p>";
            respond(&mut stream, "403 Forbidden", "text/html; charset=utf-8", "no-store", body.as_bytes())
        }
    }
}

/// Counts a phone in while it is connected.
struct Present<'a>(&'a Shared);

impl Drop for Present<'_> {
    fn drop(&mut self) {
        self.0.direct_phones.fetch_sub(1, Ordering::Relaxed);
        self.0.wake();
    }
}

/// One phone, until it goes: what it says is queued for the interface, and
/// every change of status is sent to it.
fn session(mut stream: TcpStream, shared: &Shared, rest: Vec<u8>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(40)))?;
    stream.set_nodelay(true)?;
    shared.direct_phones.fetch_add(1, Ordering::Relaxed);
    let _present = Present(shared);
    shared.wake();
    let mut reader = Reader::default();
    reader.feed(&rest);
    let mut sent = u64::MAX;
    let mut heard = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        for frame in reader.read(&mut stream)? {
            heard = Instant::now();
            match frame {
                Frame::Text(message) => match said(&message) {
                    Said::Op(op) => shared.push(op),
                    Said::Hello => sent = u64::MAX,
                    Said::Nothing => {}
                },
                Frame::Ping(payload) => stream.write_all(&ws::encode(0xA, &payload, false))?,
                Frame::Close => {
                    let _ = stream.write_all(&ws::encode(0x8, &[], false));
                    return Ok(());
                }
                Frame::Pong => {}
            }
        }
        let (version, json) = shared.status();
        if version != sent && !json.is_empty() {
            ws::send_text(&mut stream, &format!("s{json}"), false)?;
            sent = version;
        }
        // The page says something every few seconds; silence is a phone gone.
        if heard.elapsed() > Duration::from_secs(45) {
            return Ok(());
        }
    }
    Ok(())
}
