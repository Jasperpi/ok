//! AES-256-GCM, so that what the phone and the computer say through the relay
//! is read by nobody in between. The phone does the same with WebCrypto.
//!
//! Plain and unhurried: the S-box is computed rather than typed in, and
//! GHASH multiplies bit by bit. The messages are a few words long.

use std::sync::OnceLock;

/// The AES S-box: the multiplicative inverse in GF(2^8), then the affine map.
fn sbox() -> &'static [u8; 256] {
    static BOX: OnceLock<[u8; 256]> = OnceLock::new();
    BOX.get_or_init(|| {
        let mut table = [0u8; 256];
        for (x, slot) in table.iter_mut().enumerate() {
            let inv = if x == 0 { 0 } else { (1..=255u8).find(|&y| gmul(x as u8, y) == 1).unwrap_or(0) };
            let mut s = inv;
            let mut r = inv;
            for _ in 0..4 {
                r = r.rotate_left(1);
                s ^= r;
            }
            *slot = s ^ 0x63;
        }
        table
    })
}

/// Multiplication in AES's GF(2^8).
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            p ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    p
}

/// An AES-256 key, expanded.
pub struct Aes256 {
    round_keys: [[u8; 16]; 15],
}

impl Aes256 {
    pub fn new(key: &[u8; 32]) -> Aes256 {
        let s = sbox();
        let mut w = [[0u8; 4]; 60];
        for i in 0..8 {
            w[i].copy_from_slice(&key[i * 4..i * 4 + 4]);
        }
        let mut rcon = 1u8;
        for i in 8..60 {
            let mut t = w[i - 1];
            if i % 8 == 0 {
                t = [s[t[1] as usize] ^ rcon, s[t[2] as usize], s[t[3] as usize], s[t[0] as usize]];
                rcon = gmul(rcon, 2);
            } else if i % 8 == 4 {
                t = t.map(|b| s[b as usize]);
            }
            for j in 0..4 {
                w[i][j] = w[i - 8][j] ^ t[j];
            }
        }
        let mut round_keys = [[0u8; 16]; 15];
        for (r, key) in round_keys.iter_mut().enumerate() {
            for c in 0..4 {
                key[c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
            }
        }
        Aes256 { round_keys }
    }

    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let s = sbox();
        xor16(block, &self.round_keys[0]);
        for round in 1..15 {
            for b in block.iter_mut() {
                *b = s[*b as usize];
            }
            // Shift rows: row r of the column-major state moves left by r.
            let copy = *block;
            for c in 0..4 {
                for r in 0..4 {
                    block[c * 4 + r] = copy[((c + r) % 4) * 4 + r];
                }
            }
            if round != 14 {
                for c in 0..4 {
                    let col = [block[c * 4], block[c * 4 + 1], block[c * 4 + 2], block[c * 4 + 3]];
                    block[c * 4] = gmul(col[0], 2) ^ gmul(col[1], 3) ^ col[2] ^ col[3];
                    block[c * 4 + 1] = col[0] ^ gmul(col[1], 2) ^ gmul(col[2], 3) ^ col[3];
                    block[c * 4 + 2] = col[0] ^ col[1] ^ gmul(col[2], 2) ^ gmul(col[3], 3);
                    block[c * 4 + 3] = gmul(col[0], 3) ^ col[1] ^ col[2] ^ gmul(col[3], 2);
                }
            }
            xor16(block, &self.round_keys[round]);
        }
    }
}

fn xor16(a: &mut [u8; 16], b: &[u8; 16]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x ^= y;
    }
}

/// Multiplication in GCM's GF(2^128), bit by bit.
fn gf_mul(x: u128, y: u128) -> u128 {
    const R: u128 = 0xE1 << 120;
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        if x >> (127 - i) & 1 == 1 {
            z ^= v;
        }
        v = if v & 1 == 1 { (v >> 1) ^ R } else { v >> 1 };
    }
    z
}

fn ghash(h: u128, aad: &[u8], data: &[u8]) -> u128 {
    let mut y = 0u128;
    for part in [aad, data] {
        for chunk in part.chunks(16) {
            let mut block = [0u8; 16];
            block[..chunk.len()].copy_from_slice(chunk);
            y = gf_mul(y ^ u128::from_be_bytes(block), h);
        }
    }
    let lengths = ((aad.len() as u128 * 8) << 64) | (data.len() as u128 * 8);
    gf_mul(y ^ lengths, h)
}

/// The keystream applied to `data` from counter block J0 + 1.
fn ctr(aes: &Aes256, nonce: &[u8; 12], data: &mut [u8]) {
    for (i, chunk) in data.chunks_mut(16).enumerate() {
        let mut block = [0u8; 16];
        block[..12].copy_from_slice(nonce);
        block[12..].copy_from_slice(&(i as u32 + 2).to_be_bytes());
        aes.encrypt_block(&mut block);
        for (b, k) in chunk.iter_mut().zip(block) {
            *b ^= k;
        }
    }
}

fn tag(aes: &Aes256, nonce: &[u8; 12], aad: &[u8], cipher: &[u8]) -> [u8; 16] {
    let mut h = [0u8; 16];
    aes.encrypt_block(&mut h);
    let s = ghash(u128::from_be_bytes(h), aad, cipher);
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;
    aes.encrypt_block(&mut j0);
    (s ^ u128::from_be_bytes(j0)).to_be_bytes()
}

/// Encrypts `plain`: the ciphertext with the 16-byte tag after it.
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let aes = Aes256::new(key);
    let mut out = plain.to_vec();
    ctr(&aes, nonce, &mut out);
    let t = tag(&aes, nonce, aad, &out);
    out.extend_from_slice(&t);
    out
}

/// Decrypts what [`seal`] made, or `None` if it was altered or is not ours.
pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < 16 {
        return None;
    }
    let (cipher, given) = sealed.split_at(sealed.len() - 16);
    let aes = Aes256::new(key);
    let want = tag(&aes, nonce, aad, cipher);
    // Compared in full whatever differs first.
    if want.iter().zip(given).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
        return None;
    }
    let mut out = cipher.to_vec();
    ctr(&aes, nonce, &mut out);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hex;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn aes_256_matches_fips_197() {
        let key: [u8; 32] = unhex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f").try_into().unwrap();
        let mut block: [u8; 16] = unhex("00112233445566778899aabbccddeeff").try_into().unwrap();
        Aes256::new(&key).encrypt_block(&mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    #[test]
    fn gcm_matches_the_specification_test_cases() {
        let zero = [0u8; 32];
        // Test case 13: nothing to encrypt.
        assert_eq!(hex(&seal(&zero, &[0; 12], b"", b"")), "530f8afbc74536b9a963b4f1c4cb738b");
        // Test case 14: one zero block.
        assert_eq!(hex(&seal(&zero, &[0; 12], b"", &[0; 16])), "cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919");
        // Test case 15.
        let key: [u8; 32] = unhex("feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308").try_into().unwrap();
        let nonce: [u8; 12] = unhex("cafebabefacedbaddecaf888").try_into().unwrap();
        let plain = unhex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255");
        let sealed = seal(&key, &nonce, b"", &plain);
        assert_eq!(hex(&sealed), "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662898015adb094dac5d93471bdec1a502270e3cc6c");
        assert_eq!(open(&key, &nonce, b"", &sealed).unwrap(), plain);
        let mut bent = sealed.clone();
        bent[3] ^= 1;
        assert_eq!(open(&key, &nonce, b"", &bent), None);
    }
}
