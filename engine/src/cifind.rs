//! An ASCII needle found in raw bytes with its case ignored, without a lower-cased copy of
//! them: 32 positions at a time (AVX2), each tested on two of the needle's bytes — the
//! first and the last — folded on the fly (a letter's byte `| 0x20`); only the positions
//! where both agree are compared whole. Where AVX2 is missing: None from `new`, and the
//! caller lower-cases and uses memmem as before.

pub struct CiFinder {
    /// Lower case (as `view::fold` leaves an ASCII needle).
    needle: Vec<u8>,
}

impl CiFinder {
    /// For a non-empty ASCII needle, lower case, on a machine with AVX2.
    pub fn new(needle: &[u8]) -> Option<CiFinder> {
        if needle.is_empty() || !needle.is_ascii() || needle.iter().any(u8::is_ascii_uppercase) { return None; }
        #[cfg(target_arch = "x86_64")]
        if std::is_x86_feature_detected!("avx2") {
            return Some(CiFinder { needle: needle.to_vec() });
        }
        None
    }

    /// The first position where the needle starts, case ignored.
    #[inline]
    pub fn find(&self, hay: &[u8]) -> Option<usize> {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: built only where AVX2 was detected (`new`).
            unsafe { find_avx2(hay, &self.needle) }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = hay;
            unreachable!()
        }
    }
}

#[inline]
fn eq_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.iter().zip(needle).all(|(h, n)| h.to_ascii_lowercase() == *n)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn find_avx2(hay: &[u8], n: &[u8]) -> Option<usize> {
    use std::arch::x86_64::*;
    let len = n.len();
    if hay.len() < len { return None; }
    let last = hay.len() - len; // the last position the needle can start at
    let (i2, b1, b2) = (len - 1, n[0], n[len - 1]);
    // A letter's byte matches either case once 0x20 is set; any other byte matches as it is.
    let fold = |b: u8| if b.is_ascii_lowercase() { 0x20u8 } else { 0 };
    let (v1, v2) = (_mm256_set1_epi8(b1 as i8), _mm256_set1_epi8(b2 as i8));
    let (o1, o2) = (_mm256_set1_epi8(fold(b1) as i8), _mm256_set1_epi8(fold(b2) as i8));
    let p = hay.as_ptr();
    let mut i = 0;
    // 32 starting positions at once while the last one still ends inside `hay`.
    while i + 31 <= last {
        // SAFETY: i + 31 + i2 ≤ last + len - 1 < hay.len().
        let a = unsafe { _mm256_loadu_si256(p.add(i) as *const __m256i) };
        let b = unsafe { _mm256_loadu_si256(p.add(i + i2) as *const __m256i) };
        let ea = _mm256_cmpeq_epi8(_mm256_or_si256(a, o1), v1);
        let eb = _mm256_cmpeq_epi8(_mm256_or_si256(b, o2), v2);
        let mut m = _mm256_movemask_epi8(_mm256_and_si256(ea, eb)) as u32;
        while m != 0 {
            let at = i + m.trailing_zeros() as usize;
            if eq_ci(&hay[at..at + len], n) { return Some(at); }
            m &= m - 1;
        }
        i += 32;
    }
    (i..=last).find(|&at| eq_ci(&hay[at..at + len], n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_as_a_lower_cased_copy_would() {
        let Some(f) = CiFinder::new(b"lyon") else { return };
        let hays: [&[u8]; 6] = [b"", b"ly", b"LYON", b"a,b,Lyon\n", b"x LyOn y lyon", b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxLYoNz"];
        for h in hays {
            let low: Vec<u8> = h.iter().map(u8::to_ascii_lowercase).collect();
            assert_eq!(f.find(h), memchr::memmem::find(&low, b"lyon"), "{:?}", String::from_utf8_lossy(h));
        }
        // Every position, every length around the 32-byte blocks; bytes that fold onto letters by `| 0x20` only.
        let base: Vec<u8> = (0..200u32).map(|k| [b'@', b'[', b'`', b'{', b'a', b'z', b'Z', b'A', b' '][(k * 7 % 9) as usize]).collect();
        for needle in [&b"a"[..], b"za", b"a z", b"@a", b"`{"] {
            let Some(f) = CiFinder::new(needle) else { continue };
            for end in 0..base.len() {
                let h = &base[..end];
                let low: Vec<u8> = h.iter().map(u8::to_ascii_lowercase).collect();
                assert_eq!(f.find(h), memchr::memmem::find(&low, needle), "{needle:?} in {end} bytes");
            }
        }
    }
}
