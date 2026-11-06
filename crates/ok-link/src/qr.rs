//! QR codes, versions 1 to 10 at error correction level M, byte mode: enough
//! for an address of up to 213 bytes, which is all the phone page needs.
//! Built the way the standard lays it out (ISO/IEC 18004), following the
//! order of Nayuki's reference implementation.

/// A finished code: `size` by `size` modules, dark where true.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qr {
    pub size: usize,
    modules: Vec<bool>,
}

impl Qr {
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

/// Per version at level M: (error-correction codewords per block, blocks of
/// the first size, data codewords in each, blocks of the second size, data
/// codewords in each).
const LEVEL_M: [(usize, usize, usize, usize, usize); 10] = [
    (10, 1, 16, 0, 0),
    (16, 1, 28, 0, 0),
    (26, 1, 44, 0, 0),
    (18, 2, 32, 0, 0),
    (24, 2, 43, 0, 0),
    (16, 4, 27, 0, 0),
    (18, 4, 31, 0, 0),
    (22, 2, 38, 2, 39),
    (22, 3, 36, 2, 37),
    (26, 4, 43, 1, 44),
];

const ALIGNMENT: [&[usize]; 10] = [&[], &[6, 18], &[6, 22], &[6, 26], &[6, 30], &[6, 34], &[6, 22, 38], &[6, 24, 42], &[6, 26, 46], &[6, 28, 50]];

/// The code for `data`, or `None` if it is longer than version 10 holds.
pub fn encode(data: &[u8]) -> Option<Qr> {
    for version in 1..=10usize {
        let (ec, g1, d1, g2, d2) = LEVEL_M[version - 1];
        let capacity = g1 * d1 + g2 * d2;
        let count_bits = if version < 10 { 8 } else { 16 };
        if 4 + count_bits + data.len() * 8 > capacity * 8 {
            continue;
        }
        let mut bits = Bits::default();
        bits.push(0b0100, 4);
        bits.push(data.len() as u32, count_bits);
        for &b in data {
            bits.push(b as u32, 8);
        }
        let room = capacity * 8 - bits.len;
        bits.push(0, room.min(4));
        while bits.len % 8 != 0 {
            bits.push(0, 1);
        }
        let mut codewords = bits.bytes;
        let mut pad = 0xEC;
        while codewords.len() < capacity {
            codewords.push(pad);
            pad ^= 0xEC ^ 0x11;
        }
        let interleaved = interleave(&codewords, ec, g1, d1, g2, d2);
        return Some(Matrix::build(version, &interleaved));
    }
    None
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

impl Bits {
    fn push(&mut self, value: u32, count: usize) {
        for i in (0..count).rev() {
            if self.len % 8 == 0 {
                self.bytes.push(0);
            }
            if value >> i & 1 == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
    }
}

/// Multiplication in GF(256) with QR's polynomial 0x11D.
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let high = a & 0x80 != 0;
        a <<= 1;
        if high {
            a ^= 0x1D;
        }
        b >>= 1;
    }
    p
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = mul(root, 2);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= mul(d, factor);
        }
    }
    result
}

fn interleave(codewords: &[u8], ec: usize, g1: usize, d1: usize, g2: usize, d2: usize) -> Vec<u8> {
    let divisor = rs_divisor(ec);
    let mut blocks: Vec<(&[u8], Vec<u8>)> = Vec::new();
    let mut at = 0;
    for (count, len) in [(g1, d1), (g2, d2)] {
        for _ in 0..count {
            let block = &codewords[at..at + len];
            blocks.push((block, rs_remainder(block, &divisor)));
            at += len;
        }
    }
    let mut out = Vec::new();
    for i in 0..d1.max(d2) {
        for (data, _) in &blocks {
            if let Some(&b) = data.get(i) {
                out.push(b);
            }
        }
    }
    for i in 0..ec {
        for (_, ecc) in &blocks {
            out.push(ecc[i]);
        }
    }
    out
}

struct Matrix {
    size: usize,
    dark: Vec<bool>,
    function: Vec<bool>,
}

impl Matrix {
    fn build(version: usize, codewords: &[u8]) -> Qr {
        let size = version * 4 + 17;
        let mut m = Matrix { size, dark: vec![false; size * size], function: vec![false; size * size] };
        m.function_patterns(version);
        m.codewords(codewords);
        let mut best: Option<(u32, Vec<bool>)> = None;
        for mask in 0..8 {
            let mut trial = Matrix { size, dark: m.dark.clone(), function: m.function.clone() };
            trial.apply_mask(mask);
            trial.format_bits(mask);
            let score = trial.penalty();
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, trial.dark));
            }
        }
        Qr { size, modules: best.map(|(_, d)| d).unwrap_or_default() }
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.dark[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }

    fn function_patterns(&mut self, version: usize) {
        let size = self.size;
        for i in 0..size {
            self.set(6, i, i % 2 == 0);
            self.set(i, 6, i % 2 == 0);
        }
        for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if x >= 0 && y >= 0 && (x as usize) < size && (y as usize) < size {
                        let dist = dx.abs().max(dy.abs());
                        self.set(x as usize, y as usize, dist != 2 && dist != 4);
                    }
                }
            }
        }
        let positions = ALIGNMENT[version - 1];
        let last = positions.len().saturating_sub(1);
        for (i, &ay) in positions.iter().enumerate() {
            for (j, &ax) in positions.iter().enumerate() {
                if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                    continue;
                }
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let dist = dx.abs().max(dy.abs());
                        self.set((ax as i32 + dx) as usize, (ay as i32 + dy) as usize, dist != 1);
                    }
                }
            }
        }
        self.format_bits(0);
        if version >= 7 {
            let mut rem = version as u32;
            for _ in 0..12 {
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
            }
            let bits = (version as u32) << 12 | rem;
            for i in 0..18 {
                let bit = bits >> i & 1 == 1;
                let (a, b) = (size - 11 + i % 3, i / 3);
                self.set(a, b, bit);
                self.set(b, a, bit);
            }
        }
    }

    /// The fifteen format bits, both copies, for level M and `mask`.
    fn format_bits(&mut self, mask: u32) {
        let data = mask; // Level M is 00.
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = (data << 10 | rem) ^ 0x5412;
        let bit = |i: u32| bits >> i & 1 == 1;
        let size = self.size;
        for i in 0..=5 {
            self.set(8, i, bit(i as u32));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i as u32));
        }
        for i in 0..8 {
            self.set(size - 1 - i, 8, bit(i as u32));
        }
        for i in 8..15 {
            self.set(8, size - 15 + i, bit(i as u32));
        }
        self.set(8, size - 8, true);
    }

    fn codewords(&mut self, data: &[u8]) {
        let size = self.size;
        let mut i = 0usize;
        let mut right = size as i32 - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { size - 1 - vert } else { vert };
                    if !self.function[y * size + x] && i < data.len() * 8 {
                        self.dark[y * size + x] = data[i >> 3] >> (7 - (i & 7)) & 1 == 1;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn apply_mask(&mut self, mask: u32) {
        for y in 0..self.size {
            for x in 0..self.size {
                if self.function[y * self.size + x] {
                    continue;
                }
                let flip = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                if flip {
                    self.dark[y * self.size + x] ^= true;
                }
            }
        }
    }

    /// The standard's four penalty rules, lower is easier to scan.
    fn penalty(&self) -> u32 {
        let n = self.size;
        let at = |x: usize, y: usize| self.dark[y * n + x];
        let mut score = 0u32;
        // Runs of five or more of one colour, across and down.
        for horizontal in [true, false] {
            for a in 0..n {
                let mut run = 1;
                for b in 1..n {
                    let (prev, here) = if horizontal { (at(b - 1, a), at(b, a)) } else { (at(a, b - 1), at(a, b)) };
                    if prev == here {
                        run += 1;
                        if run == 5 {
                            score += 3;
                        } else if run > 5 {
                            score += 1;
                        }
                    } else {
                        run = 1;
                    }
                }
            }
        }
        // Two-by-two blocks of one colour.
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let c = at(x, y);
                if c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1) {
                    score += 3;
                }
            }
        }
        // Patterns that look like a finder: 1011101 with four light on a side.
        let finder = [true, false, true, true, true, false, true];
        for horizontal in [true, false] {
            for a in 0..n {
                for b in 0..n.saturating_sub(6) {
                    let get = |k: usize| if horizontal { at(b + k, a) } else { at(a, b + k) };
                    if (0..7).all(|k| get(k) == finder[k]) {
                        let light_before = b >= 4 && (1..=4).all(|k| !(if horizontal { at(b - k, a) } else { at(a, b - k) }));
                        let light_after = b + 11 <= n && (7..11).all(|k| !get(k));
                        if light_before || light_after {
                            score += 40;
                        }
                    }
                }
            }
        }
        // How far the share of dark modules is from half.
        let dark = self.dark.iter().filter(|&&d| d).count();
        let total = n * n;
        let k = ((dark * 20).abs_diff(total * 10)).div_ceil(total).saturating_sub(1);
        score + k as u32 * 10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_grow_with_the_address() {
        assert_eq!(encode(b"ok").unwrap().size, 21);
        assert_eq!(encode(&[b'a'; 60]).unwrap().size, 4 * 4 + 17);
        assert_eq!(encode(&[b'a'; 213]).unwrap().size, 57);
        assert!(encode(&[b'a'; 214]).is_none());
    }

    #[test]
    fn the_finders_are_in_their_corners() {
        let q = encode(b"http://192.168.1.20:7337/?t=abcdefghijklmnop").unwrap();
        for (x, y) in [(0, 0), (q.size - 7, 0), (0, q.size - 7)] {
            assert!(q.dark(x, y) && q.dark(x + 6, y + 6) && !q.dark(x + 1, y + 1) && q.dark(x + 3, y + 3));
        }
    }

    #[test]
    fn reed_solomon_matches_the_standard_example() {
        // ISO/IEC 18004 Annex I: "01234567" at 1-M.
        let data = [0x10, 0x20, 0x0C, 0x56, 0x61, 0x80, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11, 0xEC, 0x11];
        assert_eq!(rs_remainder(&data, &rs_divisor(10)), [0xA5, 0x24, 0xD4, 0xC1, 0xED, 0x36, 0xC7, 0x87, 0x2C, 0x55]);
    }
}
