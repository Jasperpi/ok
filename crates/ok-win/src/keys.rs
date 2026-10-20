//! Turning Windows virtual-key codes into the keys the interface knows.
//!
//! Nothing here calls Windows, so every rule can be tested without a window.

use ok_host::{Key, Mods};

use crate::win32::*;

/// The keys that are commands whatever is held with them: movement, Enter,
/// Tab, Backspace, Delete, Escape and F1 to F12.
pub(crate) fn named_key(vk: i32) -> Option<Key> {
    Some(match vk {
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_RETURN => Key::Enter,
        VK_TAB => Key::Tab,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_ESCAPE => Key::Escape,
        VK_F1..=VK_F12 => Key::F((vk - VK_F1 + 1) as u8),
        _ => return None,
    })
}

/// The unshifted character a printing key carries on a US keyboard, which is
/// how a chord names its key. Letters follow the virtual key, so on a layout
/// that moves the letters a chord still follows what is printed on the cap.
pub(crate) fn chord_char(vk: i32) -> Option<char> {
    const LETTER_A: i32 = b'A' as i32;
    const LETTER_Z: i32 = b'Z' as i32;
    const DIGIT_0: i32 = b'0' as i32;
    const DIGIT_9: i32 = b'9' as i32;
    const NUMPAD_9: i32 = VK_NUMPAD0 + 9;
    Some(match vk {
        LETTER_A..=LETTER_Z => (vk as u8).to_ascii_lowercase() as char,
        DIGIT_0..=DIGIT_9 => vk as u8 as char,
        VK_NUMPAD0..=NUMPAD_9 => (b'0' + (vk - VK_NUMPAD0) as u8) as char,
        VK_SPACE => ' ',
        VK_OEM_1 => ';',
        VK_OEM_PLUS => '=',
        VK_OEM_COMMA => ',',
        VK_OEM_MINUS => '-',
        VK_OEM_PERIOD => '.',
        VK_OEM_2 => '/',
        VK_OEM_3 => '`',
        VK_OEM_4 => '[',
        VK_OEM_5 => '\\',
        VK_OEM_6 => ']',
        VK_OEM_7 => '\'',
        VK_MULTIPLY => '*',
        VK_ADD => '+',
        VK_SUBTRACT => '-',
        VK_DECIMAL => '.',
        VK_DIVIDE => '/',
        _ => return None,
    })
}

/// The virtual key a script presses to produce `key`: the reverse of the two
/// tables above, for the self-test.
pub(crate) fn vk_for_key(key: Key) -> Option<i32> {
    Some(match key {
        Key::Left => VK_LEFT,
        Key::Right => VK_RIGHT,
        Key::Up => VK_UP,
        Key::Down => VK_DOWN,
        Key::Home => VK_HOME,
        Key::End => VK_END,
        Key::PageUp => VK_PRIOR,
        Key::PageDown => VK_NEXT,
        Key::Enter => VK_RETURN,
        Key::Tab => VK_TAB,
        Key::Backspace => VK_BACK,
        Key::Delete => VK_DELETE,
        Key::Escape => VK_ESCAPE,
        Key::F(n @ 1..=12) => VK_F1 + n as i32 - 1,
        Key::F(_) => return None,
        Key::Char(c) => match c.to_ascii_lowercase() {
            'a'..='z' => c.to_ascii_uppercase() as i32,
            '0'..='9' => c as i32,
            ' ' => VK_SPACE,
            ';' => VK_OEM_1,
            '=' => VK_OEM_PLUS,
            ',' => VK_OEM_COMMA,
            '-' => VK_OEM_MINUS,
            '.' => VK_OEM_PERIOD,
            '/' => VK_OEM_2,
            '`' => VK_OEM_3,
            '[' => VK_OEM_4,
            '\\' => VK_OEM_5,
            ']' => VK_OEM_6,
            '\'' => VK_OEM_7,
            '*' => VK_MULTIPLY,
            '+' => VK_ADD,
            _ => return None,
        },
    })
}

pub(crate) fn is_modifier(vk: i32) -> bool {
    matches!(
        vk,
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU
    )
}

/// What one key going down means to the interface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KeyDown {
    /// Send this key with the modifiers held.
    Command(Key),
    /// Ctrl, Alt or Shift itself: only the modifier state changed.
    Modifier,
    /// Nothing to send. Either the character will arrive as text, or the key
    /// belongs to Windows.
    Nothing,
}

/// Decides what a key press is, given what is held.
///
/// The order matters. Injected text (VK_PACKET) and keys an input method has
/// taken are never commands. With Ctrl and Alt both down nothing is a command,
/// because on many keyboards that pair is AltGr and the key is typing a
/// character. Alt F4 and Alt Space are left to Windows. A printing key is a
/// command only as part of a chord; pressed plainly or with Shift its
/// character arrives separately as text. Digits on the number pad while Alt is
/// held are an Alt code being typed, not a chord.
pub(crate) fn classify(vk: i32, mods: Mods) -> KeyDown {
    if carries_text(vk) {
        return KeyDown::Nothing;
    }
    if is_modifier(vk) {
        return KeyDown::Modifier;
    }
    if mods.ctrl && mods.alt {
        return KeyDown::Nothing;
    }
    if mods.alt && !mods.ctrl && (vk == VK_F4 || vk == VK_SPACE) {
        return KeyDown::Nothing;
    }
    if let Some(key) = named_key(vk) {
        return KeyDown::Command(key);
    }
    if mods.ctrl || mods.alt {
        let alt_code = mods.alt && (VK_NUMPAD0..=VK_NUMPAD0 + 9).contains(&vk);
        if !alt_code {
            if let Some(c) = chord_char(vk) {
                return KeyDown::Command(Key::Char(c));
            }
        }
    }
    KeyDown::Nothing
}

/// True for the two keys that are not keys at all but carriers for text: the
/// packet key, behind which characters injected by another program arrive,
/// and the key an input method reports while it has the keyboard.
pub(crate) fn carries_text(vk: i32) -> bool {
    vk == VK_PACKET || vk == VK_PROCESSKEY
}

/// What one character message means to the interface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CharIn {
    /// A character to put in at the cursor.
    Text(char),
    /// A line break that came as a character and not as a key: send Enter.
    Enter,
    /// Nothing to send: a control character whose key has already been
    /// reported, a chord's character, or the second half of a line break.
    Nothing,
}

/// Decides what a character is, given what is held.
///
/// A printing character is text, unless Ctrl alone is held: that was a chord
/// (Ctrl Space makes a space) and has been sent as one. Ctrl with Alt is
/// AltGr, and its character is typing.
///
/// A control character is never text. Enter, Tab, Backspace, Escape and the
/// Ctrl chords each produce one as well as their key, and the key is what
/// was sent. `from_key` says that is the case here: the last thing the
/// keyboard did was put a real key down, so the character is that key's own
/// (Windows queues a key's characters straight behind the key).
///
/// The exception is a line break with no key behind it. Text injected by
/// another program, and what dictation or an input method hands over,
/// arrives as characters only, so a carriage return or a line feed there is
/// the only form the line break will ever take, and it is sent as the Enter
/// key. A line feed straight after such a carriage return (`after_return`)
/// is the same line break. With Ctrl or Alt held nothing is sent: Ctrl Enter
/// is a command, and a character must never turn into one.
pub(crate) fn classify_char(ch: char, held: Mods, from_key: bool, after_return: bool) -> CharIn {
    if is_text_char(ch) {
        let chord = held.ctrl && !held.alt;
        return if chord { CharIn::Nothing } else { CharIn::Text(ch) };
    }
    let line_break = matches!(ch, '\r' | '\n') && !from_key && !held.ctrl && !held.alt;
    if line_break && !(ch == '\n' && after_return) {
        CharIn::Enter
    } else {
        CharIn::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_ALT: Mods = Mods { ctrl: true, alt: true, shift: false };

    #[test]
    fn movement_and_editing_keys_are_commands_with_any_one_modifier() {
        assert_eq!(classify(VK_RETURN, Mods::NONE), KeyDown::Command(Key::Enter));
        assert_eq!(classify(VK_TAB, Mods::SHIFT), KeyDown::Command(Key::Tab));
        assert_eq!(classify(VK_BACK, Mods::CTRL), KeyDown::Command(Key::Backspace));
        assert_eq!(classify(VK_LEFT, Mods::ALT), KeyDown::Command(Key::Left));
        assert_eq!(classify(VK_PRIOR, Mods::NONE), KeyDown::Command(Key::PageUp));
        assert_eq!(classify(VK_F1, Mods::NONE), KeyDown::Command(Key::F(1)));
        assert_eq!(classify(VK_F12, Mods::SHIFT), KeyDown::Command(Key::F(12)));
    }

    #[test]
    fn a_printing_key_is_a_command_only_in_a_chord() {
        assert_eq!(classify(b'S' as i32, Mods::NONE), KeyDown::Nothing);
        assert_eq!(classify(b'S' as i32, Mods::SHIFT), KeyDown::Nothing);
        assert_eq!(classify(b'S' as i32, Mods::CTRL), KeyDown::Command(Key::Char('s')));
        assert_eq!(classify(b'Z' as i32, Mods { ctrl: true, alt: false, shift: true }), KeyDown::Command(Key::Char('z')));
        assert_eq!(classify(VK_OEM_PLUS, Mods::CTRL), KeyDown::Command(Key::Char('=')));
        assert_eq!(classify(VK_OEM_MINUS, Mods::CTRL), KeyDown::Command(Key::Char('-')));
        assert_eq!(classify(VK_OEM_2, Mods::CTRL), KeyDown::Command(Key::Char('/')));
        assert_eq!(classify(b'A' as i32, Mods::ALT), KeyDown::Command(Key::Char('a')));
        assert_eq!(classify(VK_SPACE, Mods::CTRL), KeyDown::Command(Key::Char(' ')));
    }

    #[test]
    fn ctrl_with_alt_is_altgr_and_never_a_command() {
        assert_eq!(classify(b'E' as i32, CTRL_ALT), KeyDown::Nothing);
        assert_eq!(classify(VK_LEFT, CTRL_ALT), KeyDown::Nothing);
        assert_eq!(classify(VK_RETURN, CTRL_ALT), KeyDown::Nothing);
    }

    #[test]
    fn windows_keeps_alt_f4_alt_space_and_alt_codes() {
        assert_eq!(classify(VK_F4, Mods::ALT), KeyDown::Nothing);
        assert_eq!(classify(VK_SPACE, Mods::ALT), KeyDown::Nothing);
        assert_eq!(classify(VK_F4, Mods::NONE), KeyDown::Command(Key::F(4)));
        assert_eq!(classify(VK_NUMPAD0 + 2, Mods::ALT), KeyDown::Nothing);
        assert_eq!(classify(VK_NUMPAD0 + 2, Mods::CTRL), KeyDown::Command(Key::Char('2')));
    }

    #[test]
    fn injected_text_and_input_method_keys_are_never_commands() {
        assert_eq!(classify(VK_PACKET, Mods::NONE), KeyDown::Nothing);
        assert_eq!(classify(VK_PACKET, Mods::CTRL), KeyDown::Nothing);
        assert_eq!(classify(VK_PROCESSKEY, Mods::CTRL), KeyDown::Nothing);
    }

    #[test]
    fn printing_characters_are_text_except_as_a_ctrl_chord() {
        assert_eq!(classify_char('a', Mods::NONE, true, false), CharIn::Text('a'));
        assert_eq!(classify_char('A', Mods::SHIFT, true, false), CharIn::Text('A'));
        assert_eq!(classify_char('\u{1F600}', Mods::NONE, false, false), CharIn::Text('\u{1F600}'));
        assert_eq!(classify_char(' ', Mods::CTRL, true, false), CharIn::Nothing, "Ctrl Space was a chord");
        assert_eq!(classify_char('\u{20AC}', CTRL_ALT, true, false), CharIn::Text('\u{20AC}'), "AltGr types");
    }

    #[test]
    fn a_keys_own_control_character_is_dropped() {
        // Enter, Ctrl M, Ctrl J, Tab, Backspace, Escape: the key was the event.
        for ch in ['\r', '\n', '\t', '\u{8}', '\u{1B}', '\u{1}', '\u{7F}'] {
            for held in [Mods::NONE, Mods::CTRL, Mods::SHIFT] {
                assert_eq!(classify_char(ch, held, true, false), CharIn::Nothing, "{ch:?} with {held:?}");
            }
        }
    }

    #[test]
    fn a_line_break_with_no_key_behind_it_is_the_enter_key() {
        assert_eq!(classify_char('\r', Mods::NONE, false, false), CharIn::Enter);
        assert_eq!(classify_char('\n', Mods::NONE, false, false), CharIn::Enter);
        assert_eq!(classify_char('\r', Mods::SHIFT, false, false), CharIn::Enter);
        // Carriage return and line feed together are one line break, not two.
        assert_eq!(classify_char('\n', Mods::NONE, false, true), CharIn::Nothing);
        assert_eq!(classify_char('\r', Mods::NONE, false, true), CharIn::Enter, "two returns are two");
        // It never becomes a command, and no other control character counts.
        assert_eq!(classify_char('\r', Mods::CTRL, false, false), CharIn::Nothing);
        assert_eq!(classify_char('\n', Mods::ALT, false, false), CharIn::Nothing);
        assert_eq!(classify_char('\r', CTRL_ALT, false, false), CharIn::Nothing);
        for ch in ['\t', '\u{8}', '\u{1B}', '\u{0}', '\u{85}'] {
            assert_eq!(classify_char(ch, Mods::NONE, false, false), CharIn::Nothing, "{ch:?}");
        }
    }

    #[test]
    fn only_the_packet_and_input_method_keys_carry_text() {
        assert!(carries_text(VK_PACKET) && carries_text(VK_PROCESSKEY));
        assert!(!carries_text(VK_RETURN) && !carries_text(b'M' as i32) && !carries_text(VK_CONTROL));
    }

    #[test]
    fn modifiers_report_only_themselves() {
        for vk in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LSHIFT, VK_RCONTROL, VK_RMENU] {
            assert_eq!(classify(vk, Mods::NONE), KeyDown::Modifier);
        }
    }

    #[test]
    fn every_chord_character_maps_back_to_its_key() {
        for vk in 0..256 {
            if let Some(c) = chord_char(vk) {
                let back = vk_for_key(Key::Char(c)).expect("a key for the character");
                assert_eq!(chord_char(back), Some(c), "virtual key {vk:#x}");
            }
            if let Some(key) = named_key(vk) {
                assert_eq!(vk_for_key(key), Some(vk));
            }
        }
    }
}
