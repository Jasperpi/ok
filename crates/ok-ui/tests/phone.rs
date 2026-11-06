//! A phone on the same Wi-Fi, end to end: the page of codes, the phone
//! page served behind its token, and words sent over the WebSocket landing
//! in the text as if typed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use ok_host::Event;
use ok_ui::harness::Rig;
use ok_ui::Options;

/// The port and token in the page's address.
fn parts(url: &str) -> (u16, String) {
    let rest = url.strip_prefix("http://").expect("an http address");
    let (host, query) = rest.split_once('/').expect("a path");
    let port = host.rsplit(':').next().unwrap().parse().unwrap();
    (port, query.trim_start_matches("?t=").to_string())
}

fn get(port: u16, path: &str) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut all = Vec::new();
    stream.read_to_end(&mut all).unwrap();
    let end = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (String::from_utf8_lossy(&all[..end]).into_owned(), all[end + 4..].to_vec())
}

fn open_socket(port: u16, token: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET /ws?t={token} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="), "the RFC's own answer to its own key");
    stream
}

/// A text frame as a browser sends it: masked (with a key of zeros).
fn send(stream: &mut TcpStream, text: &str) {
    let mut frame = vec![0x81, 0x80 | text.len() as u8, 0, 0, 0, 0];
    frame.extend_from_slice(text.as_bytes());
    stream.write_all(&frame).unwrap();
}

/// One text frame from the computer.
fn receive(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).unwrap();
    let mut len = (head[1] & 0x7F) as usize;
    if len == 126 {
        let mut more = [0u8; 2];
        stream.read_exact(&mut more).unwrap();
        len = u16::from_be_bytes(more) as usize;
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).unwrap();
    String::from_utf8(body).unwrap()
}

/// Ticks the program until `done` holds, as the shell does when woken.
fn until(rig: &mut Rig, what: &str, done: impl Fn(&mut Rig) -> bool) {
    let start = Instant::now();
    while !done(rig) {
        assert!(start.elapsed() < Duration::from_secs(5), "waited for {what}; the text is {:?}", rig.own_text());
        std::thread::sleep(Duration::from_millis(15));
        rig.send(Event::Tick);
    }
}

#[test]
fn a_phone_on_the_wifi_types_into_the_text() {
    let mut rig = Rig::new(Options { phone: true, ..Options::default() });
    rig.size(1920, 1080);
    rig.type_text("Before. ");

    // The line's phone puts the codes on the page.
    rig.press("esc");
    rig.type_text("phone");
    rig.press("enter");
    let screen = rig.screen().lines().join("\n");
    assert!(screen.contains("On this Wi-Fi only"), "{screen}");
    let url = rig.app.phone_address().expect("an address on the network").to_string();
    let (port, token) = parts(&url);
    // The code is drawn: black and white in the bare rows under the words.
    let frame = rig.draw().clone();
    let black = frame.pixels().iter().filter(|&&px| px == 0).count();
    let white = frame.pixels().iter().filter(|&&px| px == 0xFFFFFF).count();
    assert!(black > 1000 && white > 1000, "{black} black and {white} white pixels");

    // The page, its font, and nothing without the token.
    let (head, page) = get(port, &format!("/?t={token}"));
    assert!(head.starts_with("HTTP/1.1 200") && String::from_utf8_lossy(&page).contains("ok Gallant"));
    let (head, font) = get(port, "/gallant.ttf");
    assert!(head.contains("font/ttf") && font.starts_with(&[0, 1, 0, 0]));
    assert!(get(port, "/?t=wrong").0.starts_with("HTTP/1.1 403"));
    assert!(get(port, "/").0.starts_with("HTTP/1.1 403"));

    // A phone connects and is told what it is typing into.
    let mut phone = open_socket(port, &token);
    send(&mut phone, "h");
    until(&mut rig, "the phone to be counted", |rig| rig.app.phones() == 1);
    rig.send(Event::Tick);
    let status = receive(&mut phone);
    assert!(status.starts_with("s{") && status.contains("\"before\":\"Before. \""), "{status}");

    // Its words go in at the cursor; the page of codes gives way to the text.
    send(&mut phone, "tWords from the phone");
    until(&mut rig, "the words", |rig| rig.own_text() == "Before. Words from the phone");
    assert!(!rig.app.line_open());
    send(&mut phone, "b6");
    send(&mut phone, "kenter");
    send(&mut phone, "tNext.");
    until(&mut rig, "the changes", |rig| rig.own_text().ends_with("Next."));
    assert_eq!(rig.own_text(), "Before. Words from the\n\nNext.");
    send(&mut phone, "kwordback");
    until(&mut rig, "a word taken back", |rig| !rig.own_text().ends_with("Next."));
    // And the phone hears about it.
    let mut heard = String::new();
    for _ in 0..10 {
        heard = receive(&mut phone);
        if heard.contains("\"words\":4") {
            break;
        }
    }
    assert!(heard.contains("\"words\":4"), "{heard}");
}
