//! The phone: the page of codes the line shows for `phone`, what a phone
//! types and says arriving as keys, and the status it is kept told.
//!
//! The link itself (servers, relay, sealing) is `ok-link`; this is only how
//! the interface meets it.

use ok_gfx::{encode_png, Rect, Rgb};
use ok_host::{Event, Host, Key, Mods};
use ok_link::qr::{self, Qr};
use ok_link::{Link, Op, Relay, Status};

use super::{text_of, App};

/// Where the relay is: the page the phone opens over HTTPS, and the address
/// this computer connects to. `OK_RELAY_PAGE` and `OK_RELAY_DESK` stand in
/// for them, to try another relay.
fn relay() -> Option<Relay> {
    const PAGE: &str = "ok.jaspermeller.com";
    const DESK: &str = "ok-relay.jaspermeller.workers.dev";
    let page = std::env::var("OK_RELAY_PAGE").unwrap_or_else(|_| PAGE.to_string());
    let desk = std::env::var("OK_RELAY_DESK").unwrap_or_else(|_| DESK.to_string());
    (!page.is_empty() && !desk.is_empty()).then_some(Relay { page, desk })
}

/// Rows of the page kept bare for the codes.
const CODE_ROWS: usize = 14;

/// How much of the text either side of the cursor the phone is shown.
const BEFORE: usize = 600;
const AFTER: usize = 200;

/// What the interface keeps for the phone.
#[derive(Default)]
pub(crate) struct Phone {
    link: Option<Link>,
    /// The codes on the page, each with the line of the page it starts on
    /// and the column it starts in.
    codes: Vec<(Qr, usize, i32)>,
    /// How many phones were connected when last looked.
    phones: usize,
    /// True while a phone's words are being applied, so that applying them
    /// does not look for more.
    applying: bool,
}

fn hex(c: Rgb) -> String {
    format!("{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

/// The last `n` characters of `text`.
fn last_chars(text: &str, n: usize) -> &str {
    match text.char_indices().rev().nth(n.saturating_sub(1)) {
        Some((at, _)) if n > 0 => &text[at..],
        _ => text,
    }
}

fn first_chars(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

impl App {
    /// Starts the link if it was ever started on this computer, so that a
    /// phone that has the page saved finds it without the line.
    pub(crate) fn phone_resume(&mut self, host: &mut dyn Host) {
        if !self.phone_allowed {
            return;
        }
        let dir = ok_link::secrets_dir(&host.local_data_dir());
        if dir.join("phone.txt").exists() {
            self.phone_start(host);
        }
    }

    fn phone_start(&mut self, host: &mut dyn Host) {
        if self.phone.link.is_some() {
            return;
        }
        let dir = ok_link::secrets_dir(&host.local_data_dir());
        let icon = ok_gfx::icon(192);
        let png = encode_png(icon.width(), icon.height(), icon.pixels());
        self.phone.link = Some(Link::start(&dir, relay(), png, host.waker()));
    }

    /// The page `phone` shows: the codes, and what each one is for. `Err`
    /// says why there is none.
    pub fn phone_page(&mut self, host: &mut dyn Host) -> Result<(), String> {
        if !self.phone_allowed {
            return Err(String::from("? The phone link is not in this build."));
        }
        self.phone_start(host);
        let Some(link) = &self.phone.link else { return Err(String::from("? The phone link could not start.")) };
        let mut addresses: Vec<(String, &str)> = Vec::new();
        if let Some(url) = link.relay_url() {
            addresses.push((url, "Anywhere, with the microphone"));
        }
        if let Some(url) = link.direct_url() {
            addresses.push((url.to_string(), "On this Wi-Fi only"));
        }
        if addresses.is_empty() {
            return Err(String::from("? This computer is on no network a phone could reach."));
        }
        let mut text = String::from("# Phone\n\nPoint the phone's camera at a code and open the page it finds. What you type or say there goes onto your text as you do it.\n\n");
        let first = text.matches('\n').count();
        text.push_str(&"\n".repeat(CODE_ROWS));
        let half = (self.prefs.measure / 2) as usize;
        let mut captions = String::new();
        let mut codes = Vec::new();
        for (i, (url, caption)) in addresses.iter().enumerate() {
            if let Some(code) = qr::encode(url.as_bytes()) {
                codes.push((code, first, (i * half) as i32));
            }
            while captions.chars().count() < i * half {
                captions.push(' ');
            }
            captions.push_str(caption);
        }
        text.push_str(&captions);
        text.push_str("\n\nIn Chrome on the phone, choose Add to home screen from the menu, and ok opens there like an app. The code is needed only once.\n\nThrough the relay your words are sealed with a key only the phone and this computer hold. On the Wi-Fi they never leave the network.");
        self.show("Phone", &text, -1);
        self.phone.codes = codes;
        Ok(())
    }

    /// The page's address on this computer's network, once the link runs.
    pub fn phone_address(&self) -> Option<&str> {
        self.phone.link.as_ref().and_then(Link::direct_url)
    }

    /// How many phones are connected.
    pub fn phones(&self) -> usize {
        self.phone.link.as_ref().map_or(0, Link::phones)
    }

    /// True while the page of codes is on the sheet.
    fn phone_page_shown(&self) -> bool {
        self.held.as_ref().is_some_and(|held| held.page.is_some() && held.name == "Phone")
    }

    /// The codes on the page of codes, as squares to fill: black on white
    /// whatever the look, because that is what every camera reads.
    pub(crate) fn phone_draw(&self) -> Vec<(Rect, Rgb)> {
        let mut out = Vec::new();
        if !self.phone_page_shown() {
            return out;
        }
        let grid = &self.grid;
        let room_h = CODE_ROWS as i32 * grid.cell_h;
        let room_w = if self.phone.codes.len() > 1 { self.prefs.measure as i32 / 2 } else { self.prefs.measure as i32 } * grid.cell_w;
        for (code, line, col) in &self.phone.codes {
            let row = self.page.paper_row(self.page.layout().first_row(*line)) - self.view.top();
            let modules = code.size as i32 + 8;
            let m = (room_h.min(room_w - grid.cell_w) / modules).max(1);
            let corner = grid.cells(grid.text.first + col, row as i32, 1, 1);
            out.push((Rect::new(corner.x, corner.y, modules * m, modules * m), Rgb(255, 255, 255)));
            for y in 0..code.size {
                for x in 0..code.size {
                    if code.dark(x, y) {
                        out.push((Rect::new(corner.x + (x as i32 + 4) * m, corner.y + (y as i32 + 4) * m, m, m), Rgb(0, 0, 0)));
                    }
                }
            }
        }
        out
    }

    /// Applies whatever phones have sent, as the keys they stand for.
    pub(crate) fn phone_input(&mut self, host: &mut dyn Host) {
        if self.phone.applying {
            return;
        }
        let Some(link) = &self.phone.link else { return };
        let ops = link.take();
        if ops.is_empty() {
            return;
        }
        self.phone.applying = true;
        // The page of codes has done its work once a phone is typing.
        if self.phone_page_shown() && self.line.is_open() {
            self.event_inner(Event::Key { key: Key::Escape, mods: Mods::NONE, repeat: false }, host);
        }
        for op in ops {
            match op {
                Op::Text(text) => {
                    for ch in text.chars() {
                        let event = match ch {
                            '\n' => Event::Key { key: Key::Enter, mods: Mods::NONE, repeat: false },
                            '\r' => continue,
                            '\t' => Event::Key { key: Key::Tab, mods: Mods::NONE, repeat: false },
                            ch => Event::Text(ch),
                        };
                        self.event_inner(event, host);
                    }
                }
                Op::Back(n) => {
                    for _ in 0..n {
                        self.event_inner(Event::Key { key: Key::Backspace, mods: Mods::NONE, repeat: false }, host);
                    }
                }
                Op::Key(key) => {
                    let (key, mods) = match key {
                        ok_link::Key::Enter => (Key::Enter, Mods::NONE),
                        ok_link::Key::Tab => (Key::Tab, Mods::NONE),
                        ok_link::Key::Left => (Key::Left, Mods::NONE),
                        ok_link::Key::Right => (Key::Right, Mods::NONE),
                        ok_link::Key::Up => (Key::Up, Mods::NONE),
                        ok_link::Key::Down => (Key::Down, Mods::NONE),
                        ok_link::Key::Home => (Key::Home, Mods::NONE),
                        ok_link::Key::End => (Key::End, Mods::NONE),
                        ok_link::Key::WordBack => (Key::Backspace, Mods::CTRL),
                        ok_link::Key::Undo => (Key::Char('z'), Mods::CTRL),
                        ok_link::Key::Redo => (Key::Char('y'), Mods::CTRL),
                    };
                    self.event_inner(Event::Key { key, mods, repeat: false }, host);
                }
            }
        }
        self.phone.applying = false;
    }

    /// Tells phones how things stand, and says when one comes or goes.
    pub(crate) fn phone_publish(&mut self) {
        let Some(phones) = self.phone.link.as_ref().map(Link::phones) else { return };
        if phones != self.phone.phones {
            let sentence = if phones > self.phone.phones { "A phone is connected. What it types goes in at the cursor." } else { "The phone has gone." };
            self.phone.phones = phones;
            self.say(sentence);
        }
        if phones == 0 {
            return;
        }
        let Some(link) = &self.phone.link else { return };
        let page = text_of(&self.held, &self.page);
        let doc = page.doc();
        let head = page.cursor.head;
        let mut before = String::new();
        let mut line = head.line;
        loop {
            let text = doc.line(line);
            let part = if line == head.line { &text[..head.col.min(text.len())] } else { text };
            before = if before.is_empty() && line == head.line { part.to_string() } else { format!("{part}\n{before}") };
            if before.chars().count() >= BEFORE || line == 0 {
                break;
            }
            line -= 1;
        }
        let here = doc.line(head.line);
        let mut after = here[head.col.min(here.len())..].to_string();
        if head.line + 1 < doc.line_count() && after.chars().count() < AFTER {
            after.push('\n');
            after.push_str(doc.line(head.line + 1));
        }
        let status = Status {
            name: self.session.name().to_string(),
            words: page.words(),
            today: self.session.words_today(),
            saved: self.session.on_disk(),
            before: last_chars(&before, BEFORE).to_string(),
            after: first_chars(&after, AFTER).to_string(),
            look: [hex(self.look.paper), hex(self.look.ink), hex(self.look.desk), hex(self.look.desk_ink)],
        };
        link.publish(&status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_of_text_are_cut_on_characters() {
        assert_eq!(last_chars("abc\u{e9}", 2), "c\u{e9}");
        assert_eq!(last_chars("ab", 5), "ab");
        assert_eq!(first_chars("\u{e9}abc", 2), "\u{e9}a");
        assert_eq!(first_chars("ab", 5), "ab");
    }
}
