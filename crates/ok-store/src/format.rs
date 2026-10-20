//! Bytes to text and back. A file's line endings and byte-order mark are
//! noticed when it is read and written back as they were; a file that is not
//! UTF-8 is recognised so that it can be shown and never rewritten.

use ok_text::doc::normalize_newlines;

/// How a file ends its lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Eol {
    /// `\n`. What a new text gets.
    #[default]
    Lf,
    /// `\r\n`.
    CrLf,
}

/// The two things about a file's bytes that the text itself does not carry.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Format {
    /// How the file ends its lines.
    pub eol: Eol,
    /// The file starts with the UTF-8 byte-order mark.
    pub bom: bool,
}

/// What reading a file's bytes gave.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Decoded {
    /// Valid UTF-8. `text` has `\n` line breaks and no byte-order mark.
    /// `mixed` says the bytes cannot be made again from the text and the
    /// format alone: the file uses more than one kind of line break, or
    /// carries more than one byte-order mark. Such a file is written back
    /// with [`encode_like`], which leaves every byte nobody edited as it is.
    Text { text: String, format: Format, mixed: bool },
    /// Not UTF-8 text: bytes that are not valid UTF-8, UTF-16 with or
    /// without a byte-order mark, or anything holding a zero byte. `lossy`
    /// is the best reading for the eye; the file must never be written from it.
    Other { lossy: String },
}

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// The bytes of a file after every byte-order mark it starts with.
fn after_marks(bytes: &[u8]) -> &[u8] {
    let mut body = bytes;
    while let Some(rest) = body.strip_prefix(BOM) {
        body = rest;
    }
    body
}

fn utf16(bytes: &[u8], little: bool) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|p| if little { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) }).collect();
    normalize_newlines(&String::from_utf16_lossy(&units)).into_owned()
}

pub(crate) fn decode(bytes: &[u8]) -> Decoded {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Decoded::Other { lossy: utf16(&bytes[2..], bytes[0] == 0xFF) };
    }
    if bytes.contains(&0) {
        // A zero byte is valid UTF-8 and is in no text anyone typed: this is
        // UTF-16 without its mark, or not text at all. Read as UTF-8 it would
        // open as an ordinary text and be written back with its line breaks
        // (CR 00 LF 00) taken apart.
        let zeros_at = |odd: usize| bytes.iter().skip(odd).step_by(2).filter(|&&b| b == 0).count();
        let (even, odd) = (zeros_at(0), zeros_at(1));
        let lossy = if bytes.len().is_multiple_of(2) && odd.max(even) * 4 >= bytes.len() {
            utf16(bytes, odd >= even)
        } else {
            normalize_newlines(&String::from_utf8_lossy(bytes)).into_owned()
        };
        return Decoded::Other { lossy };
    }
    let body = after_marks(bytes);
    let marks = (bytes.len() - body.len()) / BOM.len();
    let bom = marks > 0;
    let Ok(raw) = std::str::from_utf8(body) else {
        let lossy = normalize_newlines(&String::from_utf8_lossy(body)).into_owned();
        return Decoded::Other { lossy };
    };
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }
    let eol = if crlf > lf { Eol::CrLf } else { Eol::Lf };
    let kinds = (crlf > 0) as u8 + (lf > 0) as u8 + (cr > 0) as u8;
    let mixed = kinds > 1 || cr > 0 || marks > 1;
    Decoded::Text { text: normalize_newlines(raw).into_owned(), format: Format { eol, bom }, mixed }
}

/// Where the first `decoded` bytes of a file's text end in its raw bytes
/// (byte-order marks set aside): a `\r\n` and a lone `\r` each count as the
/// one `\n` they were read as.
fn raw_offset(body: &[u8], decoded: usize) -> usize {
    let (mut at, mut seen) = (0, 0);
    while seen < decoded && at < body.len() {
        at += if body[at] == b'\r' && body.get(at + 1) == Some(&b'\n') { 2 } else { 1 };
        seen += 1;
    }
    at
}

/// The bytes `text` is written as over a file that now holds `reference`,
/// when that file's line breaks are not all of one kind. Everything the new
/// text shares with the old at its start and at its end is copied from the
/// file byte for byte, so a line break nobody touched stays what it was; only
/// what lies between is written in the file's commoner style. If the file is
/// of one kind throughout (or is not text), this is [`encode`] with `format`.
pub(crate) fn encode_like(reference: &[u8], text: &str, format: Format) -> Vec<u8> {
    let Decoded::Text { text: old, format: found, mixed: true } = decode(reference) else { return encode(text, format) };
    let new = normalize_newlines(text);
    let (o, n) = (old.as_bytes(), new.as_bytes());
    let mut head = o.iter().zip(n).take_while(|(a, b)| a == b).count();
    let tail = o.iter().rev().zip(n.iter().rev()).take(o.len().min(n.len()) - head).take_while(|(a, b)| a == b).count();
    let body = after_marks(reference);
    let mut head_end = raw_offset(body, head);
    let tail_start = raw_offset(body, o.len() - tail);
    // A lone `\r` kept at the end of the copied start would join a `\n`
    // written straight after it into one break: write that break anew instead.
    let lf_follows = |head: usize| match n[head..n.len() - tail].first() {
        Some(&byte) => byte == b'\n' && found.eol == Eol::Lf,
        None => body.get(tail_start) == Some(&b'\n'),
    };
    while head_end > 0 && body[head_end - 1] == b'\r' && lf_follows(head) {
        head -= 1;
        head_end -= 1;
    }
    let mut out = Vec::with_capacity(reference.len() + n.len());
    out.extend_from_slice(&reference[..reference.len() - body.len()]);
    out.extend_from_slice(&body[..head_end]);
    for &byte in &n[head..n.len() - tail] {
        if byte == b'\n' && found.eol == Eol::CrLf {
            out.push(b'\r');
        }
        out.push(byte);
    }
    out.extend_from_slice(&body[tail_start..]);
    // The copied bytes and the new ones must read back as the text, whatever
    // met at the seams. If they do not, every break is written in one style.
    match decode(&out) {
        Decoded::Text { text: back, .. } if back == *new => out,
        _ => encode(text, found),
    }
}

/// The bytes a text is written as: the byte-order mark if the file had one,
/// and every line break in the file's own style.
pub(crate) fn encode(text: &str, format: Format) -> Vec<u8> {
    let text = normalize_newlines(text);
    let mut out = Vec::with_capacity(text.len() + if format.bom { 3 } else { 0 } + if format.eol == Eol::CrLf { text.len() / 32 } else { 0 });
    if format.bom {
        out.extend_from_slice(BOM);
    }
    match format.eol {
        Eol::Lf => out.extend_from_slice(text.as_bytes()),
        Eol::CrLf => {
            let mut rest = text.as_bytes();
            while let Some(at) = rest.iter().position(|&c| c == b'\n') {
                out.extend_from_slice(&rest[..at]);
                out.extend_from_slice(b"\r\n");
                rest = &rest[at + 1..];
            }
            out.extend_from_slice(rest);
        }
    }
    out
}

/// How many words `new` has that `old` did not, judged by setting aside what
/// the two share at the start and at the end and counting the words in what
/// is left of `new`. This is the number behind "Brought back 41 words".
pub fn words_added(old: &str, new: &str) -> usize {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()) - head;
    let mut tail = a.iter().rev().zip(b.iter().rev()).take(most).take_while(|(x, y)| x == y).count();
    // Where the shared part ends inside a word, widen the middle to take the
    // whole word, so half a word typed after the save counts as one.
    let space = |i: usize| b[i].is_ascii_whitespace();
    if head < b.len() && !space(head) {
        while head > 0 && !space(head - 1) {
            head -= 1;
        }
    }
    while tail > 0 && tail < b.len() && !space(b.len() - tail - 1) && !space(b.len() - tail) {
        tail -= 1;
    }
    let tail = tail.min(b.len() - head);
    String::from_utf8_lossy(&b[head..b.len() - tail]).split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(bytes: &[u8]) -> (String, Format, bool) {
        match decode(bytes) {
            Decoded::Text { text, format, mixed } => (text, format, mixed),
            Decoded::Other { .. } => panic!("expected text"),
        }
    }

    #[test]
    fn lf_round_trips_exactly() {
        let bytes = "one\ntwo\n\nthree \u{00E9}\u{4E00}\n".as_bytes();
        let (text, format, mixed) = text_of(bytes);
        assert_eq!(format, Format { eol: Eol::Lf, bom: false });
        assert!(!mixed);
        assert_eq!(text.as_bytes(), bytes);
        assert_eq!(encode(&text, format), bytes);
    }

    #[test]
    fn crlf_round_trips_exactly() {
        let bytes = b"one\r\ntwo\r\n\r\nthree\r\n";
        let (text, format, mixed) = text_of(bytes);
        assert_eq!(format, Format { eol: Eol::CrLf, bom: false });
        assert!(!mixed);
        assert_eq!(text, "one\ntwo\n\nthree\n");
        assert_eq!(encode(&text, format), bytes);
    }

    #[test]
    fn byte_order_mark_is_kept_and_not_shown() {
        let bytes = b"\xEF\xBB\xBFtitle\r\nbody";
        let (text, format, mixed) = text_of(bytes);
        assert_eq!(format, Format { eol: Eol::CrLf, bom: true });
        assert!(!mixed);
        assert_eq!(text, "title\nbody");
        assert_eq!(encode(&text, format), bytes);
        let only = text_of(b"\xEF\xBB\xBF");
        assert_eq!(only, (String::new(), Format { eol: Eol::Lf, bom: true }, false));
        assert_eq!(encode("", only.1), b"\xEF\xBB\xBF");
    }

    #[test]
    fn mixed_endings_take_the_commoner_and_say_so() {
        let (text, format, mixed) = text_of(b"a\r\nb\r\nc\nd");
        assert_eq!((text.as_str(), format.eol, mixed), ("a\nb\nc\nd", Eol::CrLf, true));
        assert_eq!(encode(&text, format), b"a\r\nb\r\nc\r\nd");
        let (text, format, mixed) = text_of(b"a\nb\nc\r\nd");
        assert_eq!((text.as_str(), format.eol, mixed), ("a\nb\nc\nd", Eol::Lf, true));
        assert_eq!(encode(&text, format), b"a\nb\nc\nd");
        // A lone carriage return is a line break too, and always counts as mixed.
        let (text, format, mixed) = text_of(b"a\rb");
        assert_eq!((text.as_str(), format.eol, mixed), ("a\nb", Eol::Lf, true));
    }

    #[test]
    fn breaks_nobody_touched_are_written_back_as_they_were() {
        let lf = Format::default();
        // Nothing changed: the very bytes, whatever they are.
        for raw in [&b"one\r\ntwo\r\nthree\nfour\r\n"[..], b"one\rtwo\rthree", b"a\r", b"\r", b"\xEF\xBB\xBF\xEF\xBB\xBFtwo marks\r\nand\n", b"a\r\r\nb\n\rc"] {
            let (text, format, mixed) = text_of(raw);
            assert!(mixed, "{raw:?}");
            assert_eq!(encode_like(raw, &text, format), raw, "{raw:?}");
        }
        // A word typed at the end: every break before it stays.
        assert_eq!(encode_like(b"one\r\ntwo\r\nthree\nfour\r\n", "one\ntwo\nthree\nfour\nfive", lf), b"one\r\ntwo\r\nthree\nfour\r\nfive");
        assert_eq!(encode_like(b"one\rtwo\rthree", "one\ntwo\nthree four", lf), b"one\rtwo\rthree four");
        // A new break takes the commoner style; the old ones around it stay.
        assert_eq!(encode_like(b"a\r\nb\r\nc\nd", "a\nb\nNEW\nc\nd", lf), b"a\r\nb\r\nNEW\r\nc\nd");
        assert_eq!(encode_like(b"a\nb\nc\r\nd", "a\nNEW\nb\nc\nd", lf), b"a\nNEW\nb\nc\r\nd");
        // Typed at the start and in the middle.
        assert_eq!(encode_like(b"a\rb\nc", "X\na\nb\nc", lf), b"X\na\rb\nc");
        assert_eq!(encode_like(b"a\rb\nc", "a\nbZ\nc", lf), b"a\rbZ\nc");
        // A break typed straight after a lone carriage return must not fuse with it.
        assert_eq!(text_of(&encode_like(b"a\rb", "a\n\nb", lf)).0, "a\n\nb");
        assert_eq!(text_of(&encode_like(b"a\rX\nb", "a\n\nb", lf)).0, "a\n\nb");
        assert_eq!(text_of(&encode_like(b"a\r\rX\nb", "a\n\n\nb", lf)).0, "a\n\n\nb");
        // Everything replaced: nothing of the old file is left to copy.
        assert_eq!(encode_like(b"a\r\nb\r\nc\nd", "wholly new\ntext", lf), b"wholly new\r\ntext");
        // A file of one kind throughout is simply encoded in the format given.
        let crlf = Format { eol: Eol::CrLf, bom: true };
        assert_eq!(encode_like(b"a\r\nb\r\n", "a\nb\nc", crlf), b"\xEF\xBB\xBFa\r\nb\r\nc");
        assert_eq!(encode_like(b"", "new\ntext", lf), b"new\ntext");
        assert_eq!(encode_like(b"caf\xE9", "x\ny", crlf), b"\xEF\xBB\xBFx\r\ny");
    }

    #[test]
    fn random_edits_to_files_of_mixed_breaks_always_read_back_as_the_text() {
        let mut seed = 0x9E37_79B9u32;
        let mut rnd = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as usize % n.max(1)
        };
        let pieces: [&[u8]; 8] = [b"\r\n", b"\n", b"\r", b"a", b"word ", b"\xC3\xA9", b"\r\r", b"\n\n"];
        for _ in 0..3000 {
            let mut raw = Vec::new();
            for _ in 0..rnd(12) {
                raw.extend_from_slice(pieces[rnd(pieces.len())]);
            }
            let Decoded::Text { text, format, .. } = decode(&raw) else { panic!("{raw:?}") };
            // One edit somewhere: some characters out, something in.
            let chars: Vec<char> = text.chars().collect();
            let at = rnd(chars.len() + 1);
            let cut = rnd(4).min(chars.len() - at);
            let put = ["", "x", "\n", "\n\n", "y\nz"][rnd(5)];
            let new: String = chars[..at].iter().collect::<String>() + put + &chars[at + cut..].iter().collect::<String>();
            let out = encode_like(&raw, &new, format);
            let Decoded::Text { text: back, .. } = decode(&out) else { panic!("{out:?}") };
            assert_eq!(back, new, "{raw:?} edited at {at}");
            if new == text {
                assert_eq!(out, if matches!(decode(&raw), Decoded::Text { mixed: true, .. }) { raw.clone() } else { encode(&text, format) }, "{raw:?}");
            }
        }
    }

    #[test]
    fn a_zero_byte_means_it_is_not_a_text_to_write() {
        let mut wide = Vec::new();
        for unit in "first line\r\nsecond\r\n".encode_utf16() {
            wide.extend_from_slice(&unit.to_le_bytes());
        }
        match decode(&wide) {
            Decoded::Other { lossy } => assert_eq!(lossy, "first line\nsecond\n"),
            other => panic!("{other:?}"),
        }
        let big: Vec<u8> = "wide".encode_utf16().flat_map(|unit| unit.to_be_bytes()).collect();
        match decode(&big) {
            Decoded::Other { lossy } => assert_eq!(lossy, "wide"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(decode(b"text with a \0 in it"), Decoded::Other { .. }));
        assert!(matches!(decode(b"\0"), Decoded::Other { .. }));
    }

    #[test]
    fn more_than_one_byte_order_mark_is_noticed_and_none_is_shown() {
        let (text, format, mixed) = text_of(b"\xEF\xBB\xBF\xEF\xBB\xBFtwice");
        assert_eq!((text.as_str(), format.bom, mixed), ("twice", true, true));
        assert_eq!(encode_like(b"\xEF\xBB\xBF\xEF\xBB\xBFtwice", "twice more", format), b"\xEF\xBB\xBF\xEF\xBB\xBFtwice more");
    }

    #[test]
    fn empty_and_single_line_files_default_to_lf() {
        assert_eq!(text_of(b""), (String::new(), Format::default(), false));
        assert_eq!(text_of(b"no break"), ("no break".to_string(), Format::default(), false));
        assert_eq!(encode("", Format::default()), b"");
    }

    #[test]
    fn invalid_utf8_is_recognised_and_still_readable() {
        match decode(b"caf\xE9 au lait\r\nline two") {
            Decoded::Other { lossy } => assert_eq!(lossy, "caf\u{FFFD} au lait\nline two"),
            other => panic!("{other:?}"),
        }
        // A truncated multi-byte character at the very end.
        assert!(matches!(decode(b"abc\xE4\xB8"), Decoded::Other { .. }));
        // Overlong and surrogate encodings are not UTF-8 either.
        assert!(matches!(decode(b"\xC0\xAF"), Decoded::Other { .. }));
        assert!(matches!(decode(b"\xED\xA0\x80"), Decoded::Other { .. }));
    }

    #[test]
    fn utf16_with_a_byte_order_mark_is_not_utf8_but_can_be_read() {
        let mut le = vec![0xFF, 0xFE];
        let mut be = vec![0xFE, 0xFF];
        for unit in "h\u{00E9}llo\r\nw".encode_utf16() {
            le.extend_from_slice(&unit.to_le_bytes());
            be.extend_from_slice(&unit.to_be_bytes());
        }
        for bytes in [le, be] {
            match decode(&bytes) {
                Decoded::Other { lossy } => assert_eq!(lossy, "h\u{00E9}llo\nw"),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn encode_never_doubles_a_carriage_return() {
        let crlf = Format { eol: Eol::CrLf, bom: false };
        assert_eq!(encode("a\r\nb\rc\nd", crlf), b"a\r\nb\r\nc\r\nd");
        assert_eq!(encode("a\r\nb", Format::default()), b"a\nb");
    }

    #[test]
    fn words_added_counts_what_is_new() {
        assert_eq!(words_added("one two", "one two three four"), 2);
        assert_eq!(words_added("one two", "one two"), 0);
        assert_eq!(words_added("", "a b c"), 3);
        assert_eq!(words_added("start end", "start some new words end"), 3);
        // Half a word typed after the save counts as a word.
        assert_eq!(words_added("the orch", "the orchard grew"), 2);
        assert_eq!(words_added("a b c", "a"), 0);
        assert_eq!(words_added("caf\u{00E9}", "caf\u{00E9} cr\u{00E8}me"), 1);
    }
}
