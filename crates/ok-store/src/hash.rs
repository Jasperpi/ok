//! Two small checksums: CRC-32 guards each journal record against a torn or
//! damaged tail, and FNV-1a 64 names a whole text so that recovery can tell
//! one of its own earlier saves from something another program wrote.

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (the zlib polynomial) over several slices as if they were one.
pub(crate) fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &byte in *part {
            crc = CRC_TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
    }
    !crc
}

/// FNV-1a, 64 bit.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// A text's fingerprint as the journal remembers it: length and FNV-1a.
pub(crate) type State = (u64, u64);

pub(crate) fn state(bytes: &[u8]) -> State {
    (bytes.len() as u64, fnv1a(bytes))
}

/// A number nobody can guess the next of, for document ids and temp names.
/// std seeds each `RandomState` from the operating system.
pub(crate) fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0x6F6B);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[b"1234", b"", b"56789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[]), 0);
    }

    #[test]
    fn fnv1a_matches_the_reference_values() {
        assert_eq!(fnv1a(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xAF63_DC4C_8601_EC8C);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_F739_67E8);
    }

    #[test]
    fn random_numbers_differ() {
        assert_ne!(random(), random());
    }
}
