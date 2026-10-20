//! A small PNG writer: 8-bit RGB, "up" filtering, one fixed-Huffman deflate
//! block with greedy matching. Interface frames are mostly flat colour, so
//! this gets them down to tens of kilobytes without any dependency.

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

const WINDOW: usize = 32768;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    /// Appends the low `count` bits of `v`, least significant first.
    fn put(&mut self, v: u32, count: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Huffman codes are written most significant bit first.
    fn code(&mut self, code: u32, count: u32) {
        self.put(code.reverse_bits() >> (32 - count), count);
    }

    /// A literal/length symbol in the fixed code of RFC 1951 section 3.2.6.
    fn symbol(&mut self, s: u32) {
        match s {
            0..=143 => self.code(0x30 + s, 8),
            144..=255 => self.code(0x190 + s - 144, 9),
            256..=279 => self.code(s - 256, 7),
            _ => self.code(0xC0 + s - 280, 8),
        }
    }

    fn pair(&mut self, len: usize, dist: usize) {
        let li = LEN_BASE.iter().rposition(|&b| b as usize <= len).unwrap();
        self.symbol(257 + li as u32);
        self.put((len - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
        let di = DIST_BASE.iter().rposition(|&b| b as usize <= dist).unwrap();
        self.code(di as u32, 5);
        self.put((dist - DIST_BASE[di] as usize) as u32, DIST_EXTRA[di] as u32);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn hash3(d: &[u8], i: usize) -> usize {
    let v = (d[i] as u32) | (d[i + 1] as u32) << 8 | (d[i + 2] as u32) << 16;
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut b = Bits { out: Vec::with_capacity(n / 8 + 64), acc: 0, n: 0 };
    b.put(1, 1); // final block
    b.put(1, 2); // fixed Huffman codes
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut i = 0;
    while i < n {
        let mut len = 0;
        let mut dist = 0;
        if i + 3 <= n {
            let h = hash3(data, i);
            let cand = head[h];
            head[h] = i;
            if cand != usize::MAX && i - cand <= WINDOW {
                let max = (n - i).min(MAX_MATCH);
                while len < max && data[cand + len] == data[i + len] {
                    len += 1;
                }
                dist = i - cand;
            }
        }
        if len >= 3 {
            b.pair(len, dist);
            // Index the bytes we skip so later matches can find them.
            let end = i + len;
            i += 1;
            while i < end {
                if i + 3 <= n {
                    head[hash3(data, i)] = i;
                }
                i += 1;
            }
        } else {
            b.symbol(data[i] as u32);
            i += 1;
        }
    }
    b.symbol(256);
    b.finish()
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = u32::MAX;
    for part in parts {
        for &x in *part {
            c = table[((c ^ x as u32) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out.extend_from_slice(&crc32(&[kind, body]).to_be_bytes());
}

/// Encodes `width * height` pixels of `0x00RRGGBB`, top row first, as a PNG.
pub fn encode_png(width: usize, height: usize, pixels: &[u32]) -> Vec<u8> {
    assert!(pixels.len() >= width * height, "pixel buffer is smaller than width * height");
    let stride = width * 3;
    let mut raw = Vec::with_capacity((stride + 1) * height);
    let mut prev = vec![0u8; stride];
    let mut cur = vec![0u8; stride];
    for y in 0..height {
        for (x, &p) in pixels[y * width..(y + 1) * width].iter().enumerate() {
            cur[x * 3] = (p >> 16) as u8;
            cur[x * 3 + 1] = (p >> 8) as u8;
            cur[x * 3 + 2] = p as u8;
        }
        raw.push(2); // filter: difference from the row above
        raw.extend(cur.iter().zip(&prev).map(|(c, p)| c.wrapping_sub(*p)));
        std::mem::swap(&mut prev, &mut cur);
    }

    let mut z = vec![0x78, 0x01];
    z.extend(deflate(&raw));
    z.extend(adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend((width as u32).to_be_bytes());
    ihdr.extend((height as u32).to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]); // 8 bits, truecolour, deflate, adaptive filters, no interlace

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference inflater for fixed-Huffman streams, so the encoder is
    /// checked by decoding what it wrote rather than by trusting itself.
    fn inflate_fixed(z: &[u8]) -> Vec<u8> {
        struct R<'a> {
            d: &'a [u8],
            pos: usize,
        }
        impl R<'_> {
            fn bit(&mut self) -> u32 {
                let b = (self.d[self.pos / 8] >> (self.pos % 8)) & 1;
                self.pos += 1;
                b as u32
            }
            fn bits(&mut self, n: u32) -> u32 {
                (0..n).fold(0, |v, i| v | self.bit() << i)
            }
            fn huff(&mut self, n: u32) -> u32 {
                (0..n).fold(0, |v, _| v << 1 | self.bit())
            }
        }
        let mut r = R { d: z, pos: 0 };
        assert_eq!(r.bits(1), 1);
        assert_eq!(r.bits(2), 1);
        let mut out = Vec::new();
        loop {
            let mut c = r.huff(7);
            let sym = if c <= 0b0010111 {
                256 + c
            } else {
                c = c << 1 | r.bit();
                if (0x30..=0xBF).contains(&c) {
                    c - 0x30
                } else if (0xC0..=0xC7).contains(&c) {
                    280 + c - 0xC0
                } else {
                    c = c << 1 | r.bit();
                    144 + c - 0x190
                }
            };
            match sym {
                0..=255 => out.push(sym as u8),
                256 => return out,
                _ => {
                    let li = (sym - 257) as usize;
                    let len = LEN_BASE[li] as usize + r.bits(LEN_EXTRA[li] as u32) as usize;
                    let di = r.huff(5) as usize;
                    let dist = DIST_BASE[di] as usize + r.bits(DIST_EXTRA[di] as u32) as usize;
                    for _ in 0..len {
                        out.push(out[out.len() - dist]);
                    }
                }
            }
        }
    }

    fn noise(n: usize, mut s: u32) -> Vec<u8> {
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as u8
            })
            .collect()
    }

    #[test]
    fn deflate_round_trips() {
        let mut cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![7],
            vec![1, 2],
            vec![0; 100_000],
            b"abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabc".to_vec(),
            (0..=255u8).cycle().take(70_000).collect(),
            noise(50_000, 0x1234_5678),
        ];
        // Long flat runs broken by noise, like a frame of text on a ground.
        let mut mixed = Vec::new();
        for i in 0..200 {
            mixed.extend(std::iter::repeat(0u8).take(900));
            mixed.extend(noise(40, i + 1));
        }
        cases.push(mixed);
        for c in cases {
            assert_eq!(inflate_fixed(&deflate(&c)), c, "len {}", c.len());
        }
    }

    #[test]
    fn flat_frames_compress_hard() {
        let px = vec![0x00E7_C9A9u32; 640 * 480];
        let png = encode_png(640, 480, &px);
        assert!(png.len() < 20_000, "{} bytes", png.len());
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
