//! minifb's keys and characters, turned into the interface's events by the
//! same rules `ok-win` follows on Windows: a printing key is a command only
//! as part of a chord, and its character then is not typed; named keys are
//! always keys; everything else arrives as text.

use minifb::Key as K;
use ok_host::{Event, Key, Mods};

/// What one key or character comes to.
pub(crate) enum Translated {
    Event(Event),
    /// Cmd Q on a Mac: the window is to close as if its button were pressed.
    Quit,
}

/// Which modifier keys are down, which keys are held (to tell a repeat from
/// a press), and whether the characters that follow belong to a chord.
#[derive(Default)]
pub(crate) struct Held {
    shift: [bool; 2],
    control: [bool; 2],
    option: [bool; 2],
    command: [bool; 2],
    down: Vec<K>,
    swallow: bool,
}

impl Held {
    /// The modifiers as the interface knows them. On a Mac, Cmd is Ctrl and
    /// Control is Alt; Option is left to the keyboard for typing.
    pub(crate) fn mods(&self) -> Mods {
        let any = |pair: [bool; 2]| pair[0] || pair[1];
        if cfg!(target_os = "macos") {
            Mods { ctrl: any(self.command), alt: any(self.control), shift: any(self.shift) }
        } else {
            Mods { ctrl: any(self.control), alt: any(self.option), shift: any(self.shift) }
        }
    }

    fn option(&self) -> bool {
        self.option[0] || self.option[1]
    }

    pub(crate) fn key(&mut self, key: K, down: bool) -> Vec<Translated> {
        let before = self.mods();
        let slot = match key {
            K::LeftShift => Some((&mut self.shift, 0)),
            K::RightShift => Some((&mut self.shift, 1)),
            K::LeftCtrl => Some((&mut self.control, 0)),
            K::RightCtrl => Some((&mut self.control, 1)),
            K::LeftAlt => Some((&mut self.option, 0)),
            K::RightAlt => Some((&mut self.option, 1)),
            K::LeftSuper => Some((&mut self.command, 0)),
            K::RightSuper => Some((&mut self.command, 1)),
            _ => None,
        };
        if let Some((pair, side)) = slot {
            pair[side] = down;
            let after = self.mods();
            return if after != before { vec![Translated::Event(Event::Mods(after))] } else { Vec::new() };
        }
        if !down {
            self.down.retain(|&held| held != key);
            return Vec::new();
        }
        let repeat = self.down.contains(&key);
        if !repeat {
            self.down.push(key);
        }
        self.swallow = false;
        let mut mods = self.mods();

        if cfg!(target_os = "macos") && mods.ctrl && key == K::Q {
            self.swallow = true;
            return vec![Translated::Quit];
        }
        if let Some(mut named) = named(key) {
            if cfg!(target_os = "macos") {
                // The Mac's own ways of moving: Cmd to the ends of the line
                // and the text, Option by word.
                if mods.ctrl {
                    match named {
                        Key::Left => (named, mods.ctrl) = (Key::Home, false),
                        Key::Right => (named, mods.ctrl) = (Key::End, false),
                        Key::Up => named = Key::Home,
                        Key::Down => named = Key::End,
                        _ => {}
                    }
                } else if self.option() && matches!(named, Key::Left | Key::Right | Key::Up | Key::Down | Key::Backspace | Key::Delete) {
                    mods.ctrl = true;
                }
            }
            self.swallow = true;
            return vec![Translated::Event(Event::Key { key: named, mods, repeat })];
        }
        // With Ctrl and Alt both down the keyboard may be typing (AltGr).
        let chord = (mods.ctrl || mods.alt) && !(mods.ctrl && mods.alt);
        if chord {
            if let Some(ch) = chord_char(key) {
                self.swallow = true;
                return vec![Translated::Event(Event::Key { key: Key::Char(ch), mods, repeat })];
            }
        }
        Vec::new()
    }

    /// A character the keyboard typed.
    pub(crate) fn text(&mut self, ch: u32) -> Vec<Translated> {
        let Some(ch) = char::from_u32(ch) else { return Vec::new() };
        // Control characters come with keys already reported; the Mac's
        // function keys arrive as characters in a private range.
        if self.swallow || ch.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&ch) {
            return Vec::new();
        }
        vec![Translated::Event(Event::Text(ch))]
    }
}

fn named(key: K) -> Option<Key> {
    Some(match key {
        K::Left => Key::Left,
        K::Right => Key::Right,
        K::Up => Key::Up,
        K::Down => Key::Down,
        K::Home => Key::Home,
        K::End => Key::End,
        K::PageUp => Key::PageUp,
        K::PageDown => Key::PageDown,
        K::Enter | K::NumPadEnter => Key::Enter,
        K::Tab => Key::Tab,
        K::Backspace => Key::Backspace,
        K::Delete => Key::Delete,
        K::Escape => Key::Escape,
        K::F1 => Key::F(1),
        K::F2 => Key::F(2),
        K::F3 => Key::F(3),
        K::F4 => Key::F(4),
        K::F5 => Key::F(5),
        K::F6 => Key::F(6),
        K::F7 => Key::F(7),
        K::F8 => Key::F(8),
        K::F9 => Key::F(9),
        K::F10 => Key::F(10),
        K::F11 => Key::F(11),
        K::F12 => Key::F(12),
        _ => return None,
    })
}

/// The character a printing key stands for in a chord: its unshifted one.
fn chord_char(key: K) -> Option<char> {
    let letters = [
        K::A, K::B, K::C, K::D, K::E, K::F, K::G, K::H, K::I, K::J, K::K, K::L, K::M, K::N, K::O, K::P, K::Q, K::R, K::S, K::T, K::U, K::V, K::W, K::X, K::Y, K::Z,
    ];
    if let Some(i) = letters.iter().position(|&letter| letter == key) {
        return Some((b'a' + i as u8) as char);
    }
    let digits = [K::Key0, K::Key1, K::Key2, K::Key3, K::Key4, K::Key5, K::Key6, K::Key7, K::Key8, K::Key9];
    let pad = [K::NumPad0, K::NumPad1, K::NumPad2, K::NumPad3, K::NumPad4, K::NumPad5, K::NumPad6, K::NumPad7, K::NumPad8, K::NumPad9];
    if let Some(i) = digits.iter().position(|&digit| digit == key).or_else(|| pad.iter().position(|&digit| digit == key)) {
        return Some((b'0' + i as u8) as char);
    }
    Some(match key {
        K::Space => ' ',
        K::Minus | K::NumPadMinus => '-',
        K::Equal | K::NumPadPlus => '=',
        K::Comma => ',',
        K::Period => '.',
        K::Slash | K::NumPadSlash => '/',
        K::Backslash => '\\',
        K::Semicolon => ';',
        K::Apostrophe => '\'',
        K::LeftBracket => '[',
        K::RightBracket => ']',
        K::Backquote => '`',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(out: Vec<Translated>) -> Vec<Event> {
        out.into_iter().filter_map(|t| if let Translated::Event(e) = t { Some(e) } else { None }).collect()
    }

    #[test]
    fn a_plain_letter_is_text_and_a_chord_is_a_key_without_its_letter() {
        let mut held = Held::default();
        assert!(events(held.key(K::A, true)).is_empty());
        assert_eq!(events(held.text('a' as u32)), [Event::Text('a')]);
        held.key(K::A, false);
        let ctrl = if cfg!(target_os = "macos") { K::LeftSuper } else { K::LeftCtrl };
        held.key(ctrl, true);
        assert_eq!(events(held.key(K::S, true)), [Event::Key { key: Key::Char('s'), mods: Mods::CTRL, repeat: false }]);
        assert!(events(held.text('s' as u32)).is_empty(), "the chord's letter is not typed");
        assert_eq!(events(held.key(K::S, true)), [Event::Key { key: Key::Char('s'), mods: Mods::CTRL, repeat: true }]);
    }

    #[test]
    fn named_keys_are_keys_and_their_characters_are_dropped() {
        let mut held = Held::default();
        assert_eq!(events(held.key(K::Enter, true)), [Event::Key { key: Key::Enter, mods: Mods::NONE, repeat: false }]);
        assert!(events(held.text('\r' as u32)).is_empty());
        assert!(events(held.text(0xF702)).is_empty());
    }

    #[test]
    fn modifiers_are_reported_as_they_change() {
        let mut held = Held::default();
        assert_eq!(events(held.key(K::LeftShift, true)), [Event::Mods(Mods::SHIFT)]);
        assert!(events(held.key(K::RightShift, true)).is_empty());
        assert!(events(held.key(K::LeftShift, false)).is_empty());
        assert_eq!(events(held.key(K::RightShift, false)), [Event::Mods(Mods::NONE)]);
    }
}
