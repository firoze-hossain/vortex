//! vortex-core/src/sha1.rs
//! Pure Rust implementation of SHA-1 (RFC 3174) for BitTorrent piece integrity and info_hash calculation.

#[derive(Clone, Debug)]
pub struct Sha1 {
    state: [u32; 5],
    count: u64,
    buffer: [u8; 64],
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Self {
            state: [
                0x67452301,
                0xEFCDAB89,
                0x98BADCFE,
                0x10325476,
                0xC3D2E1F0,
            ],
            count: 0,
            buffer: [0u8; 64],
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut idx = (self.count as usize) & 0x3F;
        self.count += data.len() as u64;

        let mut data_idx = 0;
        let mut left = data.len();

        if idx > 0 && idx + left >= 64 {
            let fill = 64 - idx;
            self.buffer[idx..64].copy_from_slice(&data[..fill]);
            let block = self.buffer;
            self.transform(&block);
            idx = 0;
            data_idx += fill;
            left -= fill;
        }

        while left >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[data_idx..data_idx + 64]);
            self.transform(&block);
            data_idx += 64;
            left -= 64;
        }

        if left > 0 {
            self.buffer[idx..idx + left].copy_from_slice(&data[data_idx..]);
        }
    }

    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.count * 8;
        let pad_idx = (self.count as usize) & 0x3F;
        let pad_len = if pad_idx < 56 { 56 - pad_idx } else { 120 - pad_idx };

        let mut pad = [0u8; 120];
        pad[0] = 0x80;
        self.update(&pad[..pad_len]);

        let bits = bit_len.to_be_bytes();
        self.update(&bits);

        let mut digest = [0u8; 20];
        for (i, word) in self.state.iter().enumerate() {
            let bytes = word.to_be_bytes();
            digest[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
        }
        digest
    }

    pub fn digest(data: &[u8]) -> [u8; 20] {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finalize()
    }

    pub fn hex(digest: &[u8; 20]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(40);
        for &b in digest {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0xF) as usize] as char);
        }
        s
    }

    pub fn from_hex(hex: &str) -> Option<[u8; 20]> {
        if hex.len() != 40 {
            return None;
        }
        let mut out = [0u8; 20];
        for i in 0..20 {
            let high = char_to_hex_nibble(hex.as_bytes()[i * 2])?;
            let low = char_to_hex_nibble(hex.as_bytes()[i * 2 + 1])?;
            out[i] = (high << 4) | low;
        }
        Some(out)
    }

    fn transform(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };

            let temp = a.rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

fn char_to_hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha1_known_vectors() {
        assert_eq!(
            Sha1::hex(&Sha1::digest(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            Sha1::hex(&Sha1::digest(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }
}
