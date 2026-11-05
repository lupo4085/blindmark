//! Key-derived material: pattern, payload encryption, CRC.
//! Byte-exact with the Python reference implementation (format "blindmark/v1").

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

pub const FORMAT: &str = "blindmark/v1";
pub const G: usize = 64; // chip grid (G x G chips per tile)
pub const NCHIPS: usize = G * G;
pub const NBITS: usize = 48; // 32-bit ID + CRC-16
pub const NSYNC: usize = NCHIPS / 2;

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <HmacSha256 as KeyInit>::new_from_slice(key).expect("hmac accepts any key length");
    for p in parts {
        m.update(p);
    }
    let out = m.finalize().into_bytes();
    let mut r = [0u8; 32];
    r.copy_from_slice(&out);
    r
}

pub fn kdf(key: &[u8], label: &str) -> [u8; 32] {
    hmac(key, &[label.as_bytes()])
}

/// HMAC-SHA256(key, label || ctr_be64) stream, ctr = 0, 1, ...
pub struct Drbg<'a> {
    key: &'a [u8],
    label: String,
    ctr: u64,
    buf: [u8; 32],
    pos: usize,
}

impl<'a> Drbg<'a> {
    pub fn new(key: &'a [u8], label: &str) -> Self {
        Drbg { key, label: label.to_string(), ctr: 0, buf: [0; 32], pos: 32 }
    }
    pub fn next_byte(&mut self) -> u8 {
        if self.pos == 32 {
            self.buf = hmac(self.key, &[self.label.as_bytes(), &self.ctr.to_be_bytes()]);
            self.ctr += 1;
            self.pos = 0;
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        b
    }
    pub fn next_u32(&mut self) -> u32 {
        let b = [self.next_byte(), self.next_byte(), self.next_byte(), self.next_byte()];
        u32::from_be_bytes(b)
    }
}

#[derive(Clone)]
pub struct Pattern {
    pub chips: Vec<f32>,    // +/-1
    pub is_sync: Vec<bool>, // half the chips
    pub bit_of: Vec<i32>,   // payload bit index for data chips, -1 for sync
}

pub fn make_pattern(key: &[u8]) -> Pattern {
    let mut st = Drbg::new(key, &format!("{FORMAT}/pattern"));
    let raw: Vec<u8> = (0..NCHIPS / 8).map(|_| st.next_byte()).collect();
    let chips: Vec<f32> = (0..NCHIPS)
        .map(|k| if (raw[k >> 3] >> (7 - (k & 7))) & 1 == 1 { 1.0 } else { -1.0 })
        .collect();
    let mut perm: Vec<usize> = (0..NCHIPS).collect();
    for i in (1..NCHIPS).rev() {
        let m = (i + 1) as u64;
        let limit = (1u64 << 32) - ((1u64 << 32) % m);
        let x = loop {
            let x = st.next_u32() as u64;
            if x < limit {
                break x;
            }
        };
        let j = (x % m) as usize;
        perm.swap(i, j);
    }
    let mut is_sync = vec![false; NCHIPS];
    let mut bit_of = vec![-1i32; NCHIPS];
    for &p in &perm[..NSYNC] {
        is_sync[p] = true;
    }
    for (t, &p) in perm[NSYNC..].iter().enumerate() {
        bit_of[p] = (t % NBITS) as i32;
    }
    Pattern { chips, is_sync, bit_of }
}

/// Chip values for one tile (row-major G x G). `bits = None` -> sync chips only.
pub fn tile_values(p: &Pattern, bits: Option<&[u8; NBITS]>) -> Vec<f32> {
    (0..NCHIPS)
        .map(|k| {
            if p.is_sync[k] {
                p.chips[k]
            } else if let Some(b) = bits {
                p.chips[k] * if b[p.bit_of[k] as usize] == 1 { 1.0 } else { -1.0 }
            } else {
                0.0
            }
        })
        .collect()
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

fn feistel(key: &[u8], x: u64, decrypt: bool) -> u64 {
    let fk = kdf(key, &format!("{FORMAT}/feistel"));
    let rf = |i: u8, h: u32| -> u32 {
        let hb = h.to_be_bytes();
        let d = hmac(&fk, &[&[i], &hb[1..4]]);
        ((d[0] as u32) << 16) | ((d[1] as u32) << 8) | d[2] as u32
    };
    let mut l = ((x >> 24) & 0xFF_FFFF) as u32;
    let mut r = (x & 0xFF_FFFF) as u32;
    if !decrypt {
        for i in 0..4u8 {
            let nl = r;
            let nr = l ^ rf(i, r);
            l = nl;
            r = nr;
        }
    } else {
        for i in (0..4u8).rev() {
            let nl = r ^ rf(i, l);
            let nr = l;
            l = nl;
            r = nr;
        }
    }
    ((l as u64) << 24) | r as u64
}

pub fn encode_payload(key: &[u8], id: u32) -> [u8; NBITS] {
    let plain = ((id as u64) << 16) | crc16(&id.to_be_bytes()) as u64;
    let c = feistel(key, plain, false);
    let mut bits = [0u8; NBITS];
    for (i, b) in bits.iter_mut().enumerate() {
        *b = ((c >> (NBITS - 1 - i)) & 1) as u8;
    }
    bits
}

pub fn decode_payload(key: &[u8], bits: &[u8; NBITS]) -> (u32, bool) {
    let c = bits.iter().fold(0u64, |acc, &b| (acc << 1) | b as u64);
    let plain = feistel(key, c, true);
    let id = (plain >> 16) as u32;
    let crc = (plain & 0xFFFF) as u16;
    (id, crc == crc16(&id.to_be_bytes()))
}

/// Short public fingerprint of a key (safe to display / store in the registry).
pub fn fingerprint(key: &[u8]) -> String {
    let d = Sha256::digest(kdf(key, &format!("{FORMAT}/fingerprint")));
    hex::encode(&d[..6])
}

pub fn generate_key() -> anyhow::Result<[u8; 32]> {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| anyhow::anyhow!("system RNG failed: {e}"))?;
    Ok(k)
}

pub fn parse_key_file(text: &str) -> anyhow::Result<Vec<u8>> {
    let k = hex::decode(text.trim())?;
    anyhow::ensure!(k.len() >= 16, "key too short");
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Vectors produced by the Python reference (key = bytes(range(32))).
    #[test]
    fn matches_python_reference() {
        let key: Vec<u8> = (0u8..32).collect();
        let p = make_pattern(&key);
        let chips16: Vec<i32> = p.chips[..16].iter().map(|&c| c as i32).collect();
        assert_eq!(chips16, vec![-1, -1, -1, -1, -1, -1, 1, 1, 1, -1, -1, -1, 1, 1, 1, -1]);
        let sync: Vec<usize> = (0..NCHIPS).filter(|&k| p.is_sync[k]).take(8).collect();
        assert_eq!(sync, vec![0, 3, 4, 5, 8, 10, 13, 14]);
        assert_eq!(p.is_sync.iter().filter(|&&s| s).count(), NSYNC);
        let bits: String = encode_payload(&key, 0x12345678).iter().map(|b| (b'0' + b) as char).collect();
        assert_eq!(bits, "110001001000110010000011110111001111101001110101");
        for id in [0u32, 1, 0xdeadbeef, u32::MAX] {
            assert_eq!(decode_payload(&key, &encode_payload(&key, id)), (id, true));
        }
    }
}
