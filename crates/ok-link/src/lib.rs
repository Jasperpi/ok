//! The phone link. A phone types and dictates into ok on the computer by one
//! of two ways, the same page either way:
//!
//! - **Direct**, on the same Wi-Fi: this crate serves the phone page and a
//!   WebSocket from the computer itself. Nothing leaves the network. The
//!   page's address carries a token, so nobody else on the Wi-Fi can type.
//! - **Through the relay**, from anywhere: the computer keeps a connection
//!   open to a small relay on the web, and the phone opens the relay's page.
//!   Everything said between them is sealed with AES-256-GCM under a key
//!   only the phone and the computer have, from the QR code; the relay only
//!   passes sealed messages along.
//!
//! What a phone says is a list of [`Op`]s for the interface to apply as if
//! typed; what the computer tells it is a [`Status`]: the text it is in, the
//! words around the cursor, whether it is saved, and the look's colours.
//!
//! Everything here is the standard library: hashes, AES-GCM, WebSocket
//! framing, QR codes and the TrueType font are written out in this crate.

pub mod gcm;
pub mod hash;
pub mod qr;
mod relay;
mod server;
pub mod ttf;
mod ws;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use hash::{base64url, hex, random_bytes, sha256};

/// Called from the link's threads when a phone has said something.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// One thing a phone asks of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Characters to put in at the cursor. A line break is Enter.
    Text(String),
    /// Characters to take away before the cursor.
    Back(usize),
    Key(Key),
}

/// The keys a phone can press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Delete the word before the cursor.
    WordBack,
    Undo,
    Redo,
}

impl Key {
    fn named(name: &str) -> Option<Key> {
        Some(match name {
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "wordback" => Key::WordBack,
            "undo" => Key::Undo,
            "redo" => Key::Redo,
            _ => return None,
        })
    }
}

/// What the phone is told about the computer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The open text's name.
    pub name: String,
    pub words: usize,
    pub today: usize,
    /// Everything typed is on disk.
    pub saved: bool,
    /// The text just before the cursor, and just after it.
    pub before: String,
    pub after: String,
    /// The look: paper, ink, desk, desk ink, as `RRGGBB`.
    pub look: [String; 4],
}

impl Status {
    fn json(&self, direct: &str, relay: &str) -> String {
        format!(
            "{{\"name\":{},\"words\":{},\"today\":{},\"saved\":{},\"before\":{},\"after\":{},\"look\":[{},{},{},{}],\"direct\":{},\"relay\":{}}}",
            quote(&self.name),
            self.words,
            self.today,
            self.saved,
            quote(&self.before),
            quote(&self.after),
            quote(&self.look[0]),
            quote(&self.look[1]),
            quote(&self.look[2]),
            quote(&self.look[3]),
            quote(direct),
            quote(relay)
        )
    }
}

/// A JSON string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Reads one message from a phone. `Hello` asks for the status at once.
pub(crate) enum Said {
    Op(Op),
    Hello,
    Nothing,
}

pub(crate) fn said(message: &str) -> Said {
    let mut chars = message.chars();
    let rest = &message[message.chars().next().map_or(0, char::len_utf8)..];
    match chars.next() {
        Some('t') if !rest.is_empty() => Said::Op(Op::Text(rest.to_string())),
        Some('b') => match rest.parse::<usize>() {
            Ok(n) if n > 0 => Said::Op(Op::Back(n.min(10_000))),
            _ => Said::Nothing,
        },
        Some('k') => Key::named(rest).map_or(Said::Nothing, |key| Said::Op(Op::Key(key))),
        Some('h') => Said::Hello,
        _ => Said::Nothing,
    }
}

/// Where the relay is.
#[derive(Clone, Debug)]
pub struct Relay {
    /// The host the phone opens, over HTTPS.
    pub page: String,
    /// The host and port the computer connects to over plain HTTP (what it
    /// says is sealed, so nothing is lost by that).
    pub desk: String,
}

/// What the link's threads share with the interface.
pub(crate) struct Shared {
    ops: Mutex<Vec<Op>>,
    /// The status as JSON, and a count that goes up when it changes.
    status: Mutex<(u64, String)>,
    pub(crate) direct_phones: AtomicUsize,
    pub(crate) relay_phones: AtomicUsize,
    pub(crate) relay_up: AtomicBool,
    pub(crate) stop: AtomicBool,
    pub(crate) token: String,
    wake: Wake,
}

impl Shared {
    pub(crate) fn push(&self, op: Op) {
        self.ops.lock().unwrap_or_else(|e| e.into_inner()).push(op);
        (self.wake)();
    }

    pub(crate) fn status(&self) -> (u64, String) {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn wake(&self) {
        (self.wake)();
    }
}

/// The running link.
pub struct Link {
    shared: Arc<Shared>,
    port: Option<u16>,
    direct: Option<String>,
    relay: Option<(Relay, [u8; 32])>,
}

/// The two secrets a link keeps between runs, so that the phone's saved page
/// goes on working: the token of the direct page and the relay's secret.
fn secrets(dir: &Path) -> (String, [u8; 32]) {
    let path = dir.join("phone.txt");
    let text = fs::read_to_string(&path).unwrap_or_default();
    let field = |name: &str| text.lines().find_map(|line| line.strip_prefix(name)).map(str::trim).map(str::to_string);
    let token = field("token=").filter(|t| t.len() >= 16).unwrap_or_else(|| base64url(&random_bytes(16)));
    let secret = field("secret=")
        .and_then(|s| hash::base64_decode(&s))
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .unwrap_or_else(|| random_bytes(32).try_into().unwrap_or([7; 32]));
    let wanted = format!("token={token}\nsecret={}\n", base64url(&secret));
    if wanted != text {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(&path, wanted);
    }
    (token, secret)
}

/// The room on the relay and the key, both from the secret.
pub(crate) fn room_and_key(secret: &[u8; 32]) -> (String, [u8; 32]) {
    let room = hex(&sha256(&[b"ok room ".as_slice(), secret].concat())[..16]);
    let key = sha256(&[b"ok key ".as_slice(), secret].concat());
    (room, key)
}

/// This computer's address on the network it reaches the world through.
fn local_address() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // Nothing is sent: connecting a UDP socket only picks the route.
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// The first port tried for the phone page; the nine after it if it is taken.
pub const PORT: u16 = 7337;

impl Link {
    /// Starts serving the phone page on this computer and, if `relay` is
    /// given, keeps a connection to the relay. `dir` is where the secrets
    /// are kept. Never fails outright: what could not start is left out.
    pub fn start(dir: &Path, relay: Option<Relay>, icon_png: Vec<u8>, wake: Wake) -> Link {
        let (token, secret) = secrets(dir);
        let shared = Arc::new(Shared {
            ops: Mutex::new(Vec::new()),
            status: Mutex::new((0, String::new())),
            direct_phones: AtomicUsize::new(0),
            relay_phones: AtomicUsize::new(0),
            relay_up: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            token: token.clone(),
            wake,
        });
        let port = server::start(shared.clone(), icon_png, relay.as_ref().map(|r| r.page.clone()));
        let direct = port.and_then(|port| local_address().map(|ip| format!("http://{ip}:{port}/?t={token}")));
        if let Some(relay) = &relay {
            relay::start(shared.clone(), relay.desk.clone(), secret);
        }
        Link { shared, port, direct, relay: relay.map(|r| (r, secret)) }
    }

    /// What phones have asked for since the last call, in order.
    pub fn take(&self) -> Vec<Op> {
        std::mem::take(&mut *self.shared.ops.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Tells every phone how things stand, if anything has changed.
    pub fn publish(&self, status: &Status) {
        let json = status.json(self.direct.as_deref().unwrap_or(""), &self.relay_url().unwrap_or_default());
        let mut held = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        if held.1 != json {
            *held = (held.0 + 1, json);
        }
    }

    /// The direct page's address, if this computer could start serving it.
    pub fn direct_url(&self) -> Option<&str> {
        self.direct.as_deref()
    }

    /// The relay page's address, with the secret in the part after `#`,
    /// which a browser never sends to the server.
    pub fn relay_url(&self) -> Option<String> {
        self.relay.as_ref().map(|(relay, secret)| format!("https://{}/#s={}", relay.page, base64url(secret)))
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// How many phones are connected, either way.
    pub fn phones(&self) -> usize {
        self.shared.direct_phones.load(Ordering::Relaxed) + self.shared.relay_phones.load(Ordering::Relaxed)
    }

    /// True while the computer is connected to the relay.
    pub fn relay_up(&self) -> bool {
        self.shared.relay_up.load(Ordering::Relaxed)
    }

    /// Forgets every phone: new token and secret, so old codes stop working.
    /// The link must be started again to use them.
    pub fn forget(dir: &Path) {
        let _ = fs::remove_file(dir.join("phone.txt"));
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// The phone page, as the relay serves it too.
pub fn page() -> &'static str {
    server::PAGE
}

/// Gallant as a TrueType font, as the phone page uses it.
pub fn font() -> &'static [u8] {
    server::font()
}

/// Where a link keeps its secrets inside the data folder.
pub fn secrets_dir(data: &Path) -> PathBuf {
    data.join("state")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_phone_says_is_read() {
        assert!(matches!(said("tHello"), Said::Op(Op::Text(t)) if t == "Hello"));
        assert!(matches!(said("t\u{e9}t\u{e9}"), Said::Op(Op::Text(t)) if t == "\u{e9}t\u{e9}"));
        assert!(matches!(said("b3"), Said::Op(Op::Back(3))));
        assert!(matches!(said("b0"), Said::Nothing));
        assert!(matches!(said("kenter"), Said::Op(Op::Key(Key::Enter))));
        assert!(matches!(said("kexplode"), Said::Nothing));
        assert!(matches!(said("h"), Said::Hello));
        assert!(matches!(said(""), Said::Nothing));
    }

    #[test]
    fn the_status_is_json() {
        let status = Status { name: "The \"orchard\"".into(), words: 3, today: 9, saved: true, before: "a\nb".into(), after: String::new(), look: ["EEE3C8".into(), "221F1B".into(), "35251A".into(), "E6D9BA".into()] };
        assert_eq!(status.json("http://x", ""), r#"{"name":"The \"orchard\"","words":3,"today":9,"saved":true,"before":"a\nb","after":"","look":["EEE3C8","221F1B","35251A","E6D9BA"],"direct":"http://x","relay":""}"#);
    }

    #[test]
    fn secrets_are_kept_between_runs() {
        let dir = std::env::temp_dir().join(format!("ok-link-secrets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let first = secrets(&dir);
        assert_eq!(secrets(&dir), first);
        Link::forget(&dir);
        assert_ne!(secrets(&dir), first);
        let _ = fs::remove_dir_all(&dir);
    }
}
