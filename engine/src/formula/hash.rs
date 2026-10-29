//! IDs and hashes (35-hash.js): every hash reads the text as UTF-8 and gives
//! lower-case hex, as md5sum, sha256sum or b2sum print it; the caller leaves an
//! empty cell empty — a hash of nothing would make every empty cell look like a value.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hash {
    Md5,
    Sha1,
    Sha256,
    Sha3_256,
    Sha3_512,
    Blake2b,
    Blake3,
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

pub fn hash(h: Hash, s: &str) -> String {
    use sha2::Digest;
    let b = s.as_bytes();
    match h {
        Hash::Md5 => hex(&md5::Md5::digest(b)),
        Hash::Sha1 => hex(&sha1::Sha1::digest(b)),
        Hash::Sha256 => hex(&sha2::Sha256::digest(b)),
        Hash::Sha3_256 => hex(&sha3::Sha3_256::digest(b)),
        Hash::Sha3_512 => hex(&sha3::Sha3_512::digest(b)),
        Hash::Blake2b => hex(&blake2::Blake2b512::digest(b)),
        Hash::Blake3 => blake3::hash(b).to_hex().to_string(),
    }
}

thread_local! {
    /// Random bytes drawn from the OS a page at a time: a column of a million uuid() is not a million system calls.
    static POOL: RefCell<(Vec<u8>, usize)> = const { RefCell::new((Vec::new(), 0)) };
}

fn random<const N: usize>() -> [u8; N] {
    POOL.with(|p| {
        let (buf, at) = &mut *p.borrow_mut();
        if buf.len() < 4096 {
            buf.resize(4096, 0);
            *at = buf.len();
        }
        if *at + N > buf.len() {
            let _ = getrandom::fill(buf);
            *at = 0;
        }
        let mut out = [0u8; N];
        out.copy_from_slice(&buf[*at..*at + N]);
        *at += N;
        out
    })
}

/// Math.random(): uniform in [0, 1).
pub fn unit() -> f64 {
    (u64::from_le_bytes(random::<8>()) >> 11) as f64 / (1u64 << 53) as f64
}

fn dashed(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// A random UUID, version 4.
pub fn uuid4() -> String {
    let mut r = random::<16>();
    r[6] = (r[6] & 0x0f) | 0x40;
    r[8] = (r[8] & 0x3f) | 0x80;
    dashed(&r)
}

/// (last millisecond, counter)
static V7: Mutex<(u64, u16)> = Mutex::new((0, 0));

/// A UUID version 7 (RFC 9562): the time in milliseconds first, so later ones
/// sort later. Several in one millisecond count up in the 12 bits after the
/// version, and past 4 096 borrow the next millisecond, so the order holds
/// within a column too.
pub fn uuid7() -> String {
    let r = random::<10>();
    let (ms, seq) = {
        let mut st = V7.lock().unwrap_or_else(|e| e.into_inner());
        let now = super::date::now() as u64;
        if now <= st.0 {
            st.1 += 1;
            if st.1 > 0xfff {
                st.0 += 1;
                st.1 = 0;
            }
        } else {
            st.0 = now;
            st.1 = u16::from_be_bytes([r[0], r[1]]) & 0x7ff; // a random start, room left to count up
        }
        *st
    };
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = 0x70 | (seq >> 8) as u8;
    b[7] = seq as u8;
    b[8] = (r[2] & 0x3f) | 0x80;
    b[9..].copy_from_slice(&r[3..10]);
    dashed(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_as_the_command_line_tools() {
        assert_eq!(hash(Hash::Md5, "abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hash(Hash::Sha1, "abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hash(Hash::Sha256, "abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hash(Hash::Sha3_256, "abc"), "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532");
        assert!(hash(Hash::Blake2b, "abc").starts_with("ba80a53f981c4d0d6a2797b69f12f6e9"));
        assert_eq!(hash(Hash::Blake3, "abc"), "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
        let (a, b) = (uuid7(), uuid7());
        assert!(a < b && a.as_bytes()[14] == b'7');
        assert_eq!(uuid4().as_bytes()[14], b'4');
        assert!((0.0..1.0).contains(&unit()));
    }
}
