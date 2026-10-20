//! Checks the vendored `gallant.hex` line by line and packs it for the
//! binary: 178 KB instead of 504 KB of text, and nothing to parse at start.
//!
//! The file is pinned to one upstream commit. The checks here are exact on
//! purpose: if a line is damaged, or the file is swapped for another
//! version, the build stops instead of shipping a face with a hole in it.
//!
//! Layout of the packed table (read by `BitmapFont::from_packed`):
//!
//! ```text
//! 0   "OKF1"
//! 4   cell width, cell height, ascent, 0
//! 8   number of runs, u16 little-endian, then two zero bytes
//! 12  runs, 6 bytes each: first code point (3 bytes, little-endian),
//!     1 if the run's glyphs are double-width, count (u16 little-endian)
//! ..  glyphs in the same order, each row at exactly the glyph's width in
//!     bits, most significant bit first, rows run together, each glyph
//!     padded to a whole byte: 33 bytes for 12 by 22, 66 for 24 by 22
//! ```

use std::env;
use std::fs;
use std::path::Path;

const WIDTH: usize = 12;
const HEIGHT: usize = 22;
/// Rows above the baseline, read off the glyphs: capitals stand on row 16.
const ASCENT: u8 = 17;
const SINGLE: usize = 5232;
const DOUBLE: usize = 88;

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let source = Path::new(&dir).join("../../assets/fonts/gallant/gallant.hex");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=build.rs");
    let text = fs::read_to_string(&source).unwrap_or_else(|e| panic!("{}: {e}", source.display()));
    let packed = pack(&text).unwrap_or_else(|e| panic!("{}: {e}", source.display()));
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("gallant.bin");
    fs::write(&out, packed).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}

fn pack(text: &str) -> Result<Vec<u8>, String> {
    let lower_hex = |s: &str| s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let mut lines = text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).enumerate();

    for want in ["# Width: 12", "# Height: 22"] {
        match lines.next() {
            Some((_, line)) if line == want => {}
            other => return Err(format!("expected {want:?} before the first glyph, found {:?}", other.map(|(_, l)| l))),
        }
    }

    // (first code point, double-width, count)
    let mut runs: Vec<(u32, bool, u16)> = Vec::new();
    let mut bits: Vec<u8> = Vec::new();
    let mut last: Option<u32> = None;
    let (mut single, mut double) = (0usize, 0usize);

    for (i, line) in lines {
        let n = i + 1;
        let (cp, data) = line.split_once(':').ok_or(format!("line {n}: no colon"))?;
        if cp.len() != 4 || !lower_hex(cp) {
            return Err(format!("line {n}: the code point is not 4 lower-case hex digits"));
        }
        let cp = u32::from_str_radix(cp, 16).map_err(|e| format!("line {n}: {e}"))?;
        if last.is_some_and(|l| cp <= l) {
            return Err(format!("line {n}: code points do not ascend"));
        }
        if char::from_u32(cp).is_none() {
            return Err(format!("line {n}: U+{cp:04X} is not a character"));
        }
        let wide = match data.len() {
            len if len == HEIGHT * 4 => false,
            len if len == HEIGHT * 6 => true,
            len => return Err(format!("line {n}: {len} hex digits, expected 88 or 132")),
        };
        if !lower_hex(data) {
            return Err(format!("line {n}: glyph data is not lower-case hex"));
        }

        let (digits, width) = if wide { (6, 2 * WIDTH) } else { (4, WIDTH) };
        let start = bits.len();
        bits.resize(start + (width * HEIGHT).div_ceil(8), 0);
        let mut at = 0usize;
        for r in 0..HEIGHT {
            let row = u32::from_str_radix(&data[r * digits..(r + 1) * digits], 16).map_err(|e| format!("line {n}: {e}"))?;
            if !wide && row & 0xF != 0 {
                return Err(format!("line {n}: row {r} has ink in its pad bits"));
            }
            let row = if wide { row } else { row >> 4 };
            for b in (0..width).rev() {
                if row >> b & 1 == 1 {
                    bits[start + at / 8] |= 0x80 >> (at % 8);
                }
                at += 1;
            }
        }

        match runs.last_mut() {
            Some((first, w, count)) if *w == wide && *first + *count as u32 == cp && *count < u16::MAX => *count += 1,
            _ => runs.push((cp, wide, 1)),
        }
        if wide {
            double += 1;
        } else {
            single += 1;
        }
        last = Some(cp);
    }

    if (single, double) != (SINGLE, DOUBLE) {
        return Err(format!(
            "{single} single-width and {double} double-width glyphs, expected {SINGLE} and {DOUBLE}: this is not the pinned file"
        ));
    }
    let run_count = u16::try_from(runs.len()).map_err(|_| "too many runs".to_string())?;

    let mut out = b"OKF1".to_vec();
    out.extend([WIDTH as u8, HEIGHT as u8, ASCENT, 0]);
    out.extend(run_count.to_le_bytes());
    out.extend([0, 0]);
    for (first, wide, count) in runs {
        out.extend(&first.to_le_bytes()[..3]);
        out.push(wide as u8);
        out.extend(count.to_le_bytes());
    }
    out.extend(bits);
    Ok(out)
}
