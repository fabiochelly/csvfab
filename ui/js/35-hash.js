/* ---------------------------------------------------------------
   IDS AND HASHES (formula helpers: uuid, uuid7, md5, sha1, sha256,
   sha3_256, sha3_512, blake2b, blake3 — registered in FX, 17-…)
   Written out here rather than taken from crypto.subtle: a formula
   runs row by row and synchronously, and SubtleCrypto only answers
   with promises (it has no MD5, SHA-3 nor BLAKE anyway). Every hash
   reads the text as UTF-8 and returns lower-case hex, as md5sum,
   sha256sum or b2sum print it; an empty cell stays empty — a hash of
   nothing would make every empty cell look like a value.
   Checked against Node's crypto (md5, sha1, sha256, sha3-256,
   sha3-512, blake2b512) and the blake3 reference, on lengths around
   every block and chunk boundary.
----------------------------------------------------------------*/
function fxBytes(s) { return (fxBytes.enc || (fxBytes.enc = new TextEncoder())).encode(cellStr(s)); }
function fxHex(words, n, little) {           // the first n bytes of 32-bit words, as hex
    let out = '';
    for (let i = 0; i < n; i++) {
        const w = words[i >> 2], sh = little ? (i & 3) * 8 : 24 - (i & 3) * 8;
        out += ((w >>> sh) & 255).toString(16).padStart(2, '0');
    }
    return out;
}
function fxHashOf(fn) { return s => { const v = cellStr(s); return v === '' ? '' : fn(fxBytes(v)); }; }

/* Merkle–Damgård padding for MD5 / SHA-1 / SHA-256: 0x80, zeros, the bit length in 8 bytes. */
function mdPad(b, little) {
    const n = b.length, len = ((n + 8) >> 6) + 1 << 6, p = new Uint8Array(len);
    p.set(b); p[n] = 0x80;
    const bits = n * 8, hi = Math.floor(bits / 4294967296), lo = bits >>> 0;
    const dv = new DataView(p.buffer);
    if (little) { dv.setUint32(len - 8, lo, true); dv.setUint32(len - 4, hi, true); }
    else { dv.setUint32(len - 8, hi); dv.setUint32(len - 4, lo); }
    return dv;
}

const MD5_S = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
const MD5_K = Array.from({ length: 64 }, (_, i) => Math.floor(Math.abs(Math.sin(i + 1)) * 4294967296) >>> 0);
function md5Hex(b) {
    const dv = mdPad(b, true), h = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], m = new Array(16);
    for (let o = 0; o < dv.byteLength; o += 64) {
        for (let i = 0; i < 16; i++) m[i] = dv.getUint32(o + i * 4, true);
        let [a, bb, c, d] = h;
        for (let i = 0; i < 64; i++) {
            let f, g;
            if (i < 16) { f = (bb & c) | (~bb & d); g = i; }
            else if (i < 32) { f = (d & bb) | (~d & c); g = (5 * i + 1) & 15; }
            else if (i < 48) { f = bb ^ c ^ d; g = (3 * i + 5) & 15; }
            else { f = c ^ (bb | ~d); g = (7 * i) & 15; }
            const s = MD5_S[(i >> 4) * 4 + (i & 3)], x = (a + f + MD5_K[i] + m[g]) | 0;
            a = d; d = c; c = bb; bb = (bb + ((x << s) | (x >>> (32 - s)))) | 0;
        }
        h[0] = (h[0] + a) | 0; h[1] = (h[1] + bb) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0;
    }
    return fxHex(h, 16, true);
}

function sha1Hex(b) {
    const dv = mdPad(b, false), h = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0], w = new Int32Array(80);
    for (let o = 0; o < dv.byteLength; o += 64) {
        for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4);
        for (let i = 16; i < 80; i++) { const x = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]; w[i] = (x << 1) | (x >>> 31); }
        let [a, bb, c, d, e] = h;
        for (let i = 0; i < 80; i++) {
            const f = i < 20 ? (bb & c) | (~bb & d) : i < 40 ? bb ^ c ^ d : i < 60 ? (bb & c) | (bb & d) | (c & d) : bb ^ c ^ d;
            const k = i < 20 ? 0x5a827999 : i < 40 ? 0x6ed9eba1 : i < 60 ? 0x8f1bbcdc : 0xca62c1d6;
            const t = (((a << 5) | (a >>> 27)) + f + e + k + w[i]) | 0;
            e = d; d = c; c = (bb << 30) | (bb >>> 2); bb = a; a = t;
        }
        h[0] = (h[0] + a) | 0; h[1] = (h[1] + bb) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0; h[4] = (h[4] + e) | 0;
    }
    return fxHex(h, 20, false);
}

const SHA256_K = [0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2];
const SHA256_IV = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];   // BLAKE3's IV too
function sha256Hex(b) {
    const dv = mdPad(b, false), h = SHA256_IV.slice(), w = new Int32Array(64);
    const r = (x, n) => (x >>> n) | (x << (32 - n));
    for (let o = 0; o < dv.byteLength; o += 64) {
        for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4);
        for (let i = 16; i < 64; i++) {
            const a = w[i - 15], c = w[i - 2];
            w[i] = (w[i - 16] + (r(a, 7) ^ r(a, 18) ^ (a >>> 3)) + w[i - 7] + (r(c, 17) ^ r(c, 19) ^ (c >>> 10))) | 0;
        }
        let [a, bb, c, d, e, f, g, hh] = h;
        for (let i = 0; i < 64; i++) {
            const t1 = (hh + (r(e, 6) ^ r(e, 11) ^ r(e, 25)) + ((e & f) ^ (~e & g)) + SHA256_K[i] + w[i]) | 0;
            const t2 = ((r(a, 2) ^ r(a, 13) ^ r(a, 22)) + ((a & bb) ^ (a & c) ^ (bb & c))) | 0;
            hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = bb; bb = a; a = (t1 + t2) | 0;
        }
        h[0] = (h[0] + a) | 0; h[1] = (h[1] + bb) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0;
        h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + g) | 0; h[7] = (h[7] + hh) | 0;
    }
    return fxHex(h, 32, false);
}

/* SHA-3 (Keccak-f[1600]): 25 lanes of 64 bits as pairs of 32-bit words, low word first — so the
   state's bytes, seen through a Uint8Array, are the little-endian lanes the sponge absorbs (every
   platform a browser runs on is little-endian). */
const KECCAK_RHO = [0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14];
const KECCAK_RC = ['0000000000000001', '0000000000008082', '800000000000808a', '8000000080008000', '000000000000808b', '0000000080000001', '8000000080008081', '8000000000008009',
    '000000000000008a', '0000000000000088', '0000000080008009', '000000008000000a', '000000008000808b', '800000000000008b', '8000000000008089', '8000000000008003',
    '8000000000008002', '8000000000000080', '000000000000800a', '800000008000000a', '8000000080008081', '8000000000008080', '0000000080000001', '8000000080008008']
    .map(x => [parseInt(x.slice(8), 16), parseInt(x.slice(0, 8), 16)]);
function keccakF(s) {
    const C = new Int32Array(10), B = new Int32Array(50);
    for (let round = 0; round < 24; round++) {
        for (let x = 0; x < 5; x++) {
            C[2 * x] = s[2 * x] ^ s[2 * x + 10] ^ s[2 * x + 20] ^ s[2 * x + 30] ^ s[2 * x + 40];
            C[2 * x + 1] = s[2 * x + 1] ^ s[2 * x + 11] ^ s[2 * x + 21] ^ s[2 * x + 31] ^ s[2 * x + 41];
        }
        for (let x = 0; x < 5; x++) {
            const p = ((x + 4) % 5) * 2, q = ((x + 1) % 5) * 2;
            const lo = C[p] ^ ((C[q] << 1) | (C[q + 1] >>> 31)), hi = C[p + 1] ^ ((C[q + 1] << 1) | (C[q] >>> 31));
            for (let y = 0; y < 25; y += 5) { s[2 * (x + y)] ^= lo; s[2 * (x + y) + 1] ^= hi; }
        }
        for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) {
            const i = x + 5 * y, j = y + 5 * ((2 * x + 3 * y) % 5);
            let lo = s[2 * i], hi = s[2 * i + 1], n = KECCAK_RHO[i];
            if (n >= 32) { const t = lo; lo = hi; hi = t; n -= 32; }
            B[2 * j] = n ? (lo << n) | (hi >>> (32 - n)) : lo;
            B[2 * j + 1] = n ? (hi << n) | (lo >>> (32 - n)) : hi;
        }
        for (let y = 0; y < 25; y += 5) for (let x = 0; x < 5; x++) {
            const i = 2 * (x + y), a = 2 * ((x + 1) % 5 + y), b = 2 * ((x + 2) % 5 + y);
            s[i] = B[i] ^ (~B[a] & B[b]); s[i + 1] = B[i + 1] ^ (~B[a + 1] & B[b + 1]);
        }
        s[0] ^= KECCAK_RC[round][0]; s[1] ^= KECCAK_RC[round][1];
    }
}
function sha3Hex(b, outBytes) {
    const rate = 200 - 2 * outBytes, s = new Int32Array(50), sb = new Uint8Array(s.buffer);
    const n = b.length, full = n - (n % rate);
    for (let o = 0; o < full; o += rate) { for (let i = 0; i < rate; i++) sb[i] ^= b[o + i]; keccakF(s); }
    for (let i = full; i < n; i++) sb[i - full] ^= b[i];
    sb[n - full] ^= 0x06; sb[rate - 1] ^= 0x80;
    keccakF(s);
    let out = ''; for (let i = 0; i < outBytes; i++) out += sb[i].toString(16).padStart(2, '0');
    return out;
}

/* BLAKE2b-512 (RFC 7693), as b2sum prints it: 64-bit words as pairs of 32-bit ones, low first. */
const B2B_IV = [0xf3bcc908, 0x6a09e667, 0x84caa73b, 0xbb67ae85, 0xfe94f82b, 0x3c6ef372, 0x5f1d36f1, 0xa54ff53a,
    0xade682d1, 0x510e527f, 0x2b3e6c1f, 0x9b05688c, 0xfb41bd6b, 0x1f83d9ab, 0x137e2179, 0x5be0cd19];
const B2B_SIGMA = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3, 11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4,
    7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8, 9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13, 2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9,
    12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11, 13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10, 6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5,
    10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0];
function blake2bHex(b) {
    const h = new Uint32Array(B2B_IV), v = new Uint32Array(32), m = new Uint32Array(32);
    h[0] ^= 0x01010040;                       // no key, 64-byte digest
    const add = (a, b0, b1) => { const lo = v[a] + b0; v[a + 1] = v[a + 1] + b1 + (lo >= 4294967296 ? 1 : 0); v[a] = lo; };
    const G = (a, b, c, d, x, y) => {
        add(a, v[b], v[b + 1]); add(a, m[x], m[x + 1]);
        let lo = v[d] ^ v[a], hi = v[d + 1] ^ v[a + 1]; v[d] = hi; v[d + 1] = lo;                                   // >>> 32
        add(c, v[d], v[d + 1]);
        lo = v[b] ^ v[c]; hi = v[b + 1] ^ v[c + 1]; v[b] = (lo >>> 24) ^ (hi << 8); v[b + 1] = (hi >>> 24) ^ (lo << 8);   // >>> 24
        add(a, v[b], v[b + 1]); add(a, m[y], m[y + 1]);
        lo = v[d] ^ v[a]; hi = v[d + 1] ^ v[a + 1]; v[d] = (lo >>> 16) ^ (hi << 16); v[d + 1] = (hi >>> 16) ^ (lo << 16); // >>> 16
        add(c, v[d], v[d + 1]);
        lo = v[b] ^ v[c]; hi = v[b + 1] ^ v[c + 1]; v[b] = (hi >>> 31) ^ (lo << 1); v[b + 1] = (lo >>> 31) ^ (hi << 1);  // >>> 63
    };
    const n = b.length, blocks = Math.max(1, Math.ceil(n / 128)), blk = new Uint8Array(128), dv = new DataView(blk.buffer);
    for (let k = 0; k < blocks; k++) {
        const last = k === blocks - 1, end = Math.min(n, (k + 1) * 128);
        blk.fill(0); blk.set(b.subarray(k * 128, end));
        for (let i = 0; i < 32; i++) m[i] = dv.getUint32(i * 4, true);
        for (let i = 0; i < 16; i++) { v[i] = h[i]; v[i + 16] = B2B_IV[i]; }
        v[24] ^= end >>> 0; v[25] ^= Math.floor(end / 4294967296);
        if (last) { v[28] = ~v[28]; v[29] = ~v[29]; }
        for (let r = 0; r < 12; r++) {
            const s = (r % 10) * 16, S = i => B2B_SIGMA[s + i] * 2;
            G(0, 8, 16, 24, S(0), S(1)); G(2, 10, 18, 26, S(2), S(3)); G(4, 12, 20, 28, S(4), S(5)); G(6, 14, 22, 30, S(6), S(7));
            G(0, 10, 20, 30, S(8), S(9)); G(2, 12, 22, 24, S(10), S(11)); G(4, 14, 16, 26, S(12), S(13)); G(6, 8, 18, 28, S(14), S(15));
        }
        for (let i = 0; i < 16; i++) h[i] ^= v[i] ^ v[i + 16];
    }
    return fxHex(h, 64, true);
}

/* BLAKE3, 256 bits: chunks of 1 KB in blocks of 64 bytes, chained into a binary tree. */
const B3_PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
function b3Compress(cv, m, ctr, len, flags) {
    const s = new Uint32Array([...cv, SHA256_IV[0], SHA256_IV[1], SHA256_IV[2], SHA256_IV[3], ctr >>> 0, Math.floor(ctr / 4294967296), len, flags]);
    const r = (x, n) => (x >>> n) | (x << (32 - n));
    const g = (a, b, c, d, x, y) => {
        s[a] = s[a] + s[b] + x; s[d] = r(s[d] ^ s[a], 16); s[c] = s[c] + s[d]; s[b] = r(s[b] ^ s[c], 12);
        s[a] = s[a] + s[b] + y; s[d] = r(s[d] ^ s[a], 8); s[c] = s[c] + s[d]; s[b] = r(s[b] ^ s[c], 7);
    };
    let w = Array.from(m);
    for (let round = 0; round < 7; round++) {
        g(0, 4, 8, 12, w[0], w[1]); g(1, 5, 9, 13, w[2], w[3]); g(2, 6, 10, 14, w[4], w[5]); g(3, 7, 11, 15, w[6], w[7]);
        g(0, 5, 10, 15, w[8], w[9]); g(1, 6, 11, 12, w[10], w[11]); g(2, 7, 8, 13, w[12], w[13]); g(3, 4, 9, 14, w[14], w[15]);
        w = B3_PERM.map(i => w[i]);
    }
    for (let i = 0; i < 8; i++) { s[i] ^= s[i + 8]; s[i + 8] ^= cv[i]; }
    return s;
}
function blake3Hex(b) {
    const START = 1, END = 2, PARENT = 4, ROOT = 8, n = b.length, chunks = Math.max(1, Math.ceil(n / 1024));
    const words = (bytes) => { const p = new Uint8Array(64); p.set(bytes); const dv = new DataView(p.buffer); return Array.from({ length: 16 }, (_, i) => dv.getUint32(i * 4, true)); };
    const stack = [];
    let out = null;                            // the last node, compressed only at the end (with ROOT if it is the root)
    for (let c = 0; c < chunks; c++) {
        const chunk = b.subarray(c * 1024, Math.min(n, (c + 1) * 1024)), nb = Math.max(1, Math.ceil(chunk.length / 64));
        let cv = SHA256_IV;
        for (let k = 0; k < nb - 1; k++) cv = b3Compress(cv, words(chunk.subarray(k * 64, k * 64 + 64)), c, 64, k === 0 ? START : 0).subarray(0, 8);
        const lastB = chunk.subarray((nb - 1) * 64);
        out = { cv, m: words(lastB), ctr: c, len: lastB.length, flags: (nb === 1 ? START : 0) | END };
        if (c === chunks - 1) break;
        let cvChunk = b3Compress(out.cv, out.m, out.ctr, out.len, out.flags).subarray(0, 8), total = c + 1;
        while ((total & 1) === 0) { cvChunk = b3Compress(SHA256_IV, [...stack.pop(), ...cvChunk], 0, 64, PARENT).subarray(0, 8); total >>= 1; }
        stack.push(cvChunk);
    }
    while (stack.length) {
        const right = b3Compress(out.cv, out.m, out.ctr, out.len, out.flags).subarray(0, 8);
        out = { cv: SHA256_IV, m: [...stack.pop(), ...right], ctr: 0, len: 64, flags: PARENT };
    }
    return fxHex(b3Compress(out.cv, out.m, out.ctr, out.len, out.flags | ROOT), 32, true);
}

/* UUIDs: v4 is random; v7 (RFC 9562) starts with the time in milliseconds, so values made later
   sort later — a key for a database index. Several in the same millisecond (a computed column
   fills thousands at once) count up in the 12 bits after the version, and past 4 096 borrow
   the next millisecond, so the order holds within a column too. */
function uuid4() {
    if (crypto.randomUUID) return crypto.randomUUID();
    const r = crypto.getRandomValues(new Uint8Array(16)); r[6] = (r[6] & 15) | 64; r[8] = (r[8] & 63) | 128;
    const h = fxHex(Array.from({ length: 4 }, (_, i) => r[4 * i] << 24 | r[4 * i + 1] << 16 | r[4 * i + 2] << 8 | r[4 * i + 3]), 16, false);
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
function uuid7() {
    const st = uuid7.st || (uuid7.st = { ms: 0, seq: 0 }), r = crypto.getRandomValues(new Uint8Array(10));
    let ms = Date.now();
    if (ms <= st.ms) { ms = st.ms; if (++st.seq > 0xfff) { ms = ++st.ms; st.seq = 0; } }
    else { st.ms = ms; st.seq = ((r[0] << 8) | r[1]) & 0x7ff; }   // a random start, room left to count up
    const hex = (x, n) => x.toString(16).padStart(n, '0');
    const t = hex(ms, 12), tail = Array.from(r.subarray(2), x => hex(x, 2)).join('');
    return `${t.slice(0, 8)}-${t.slice(8)}-7${hex(st.seq, 3)}-${hex((r[2] & 0x3f) | 0x80, 2)}${tail.slice(2, 4)}-${tail.slice(4, 16)}`;
}
