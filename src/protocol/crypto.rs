//! Hay Day build 1598 crypto: X25519, HChaCha22 and XChaCha18/Poly1305.
//! Round counts are intentionally version-specific, verified against native code.
use anyhow::{bail, Result};
use blake2::{digest::consts::U24, Blake2b, Digest};
use poly1305::{universal_hash::KeyInit, Key, Poly1305};

pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0; N];
    getrandom::fill(&mut out).map_err(|e| anyhow::anyhow!("OS randomness: {e}"))?;
    Ok(out)
}

fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    x[a] = x[a].wrapping_add(x[b]);
    x[d] = (x[d] ^ x[a]).rotate_left(16);
    x[c] = x[c].wrapping_add(x[d]);
    x[b] = (x[b] ^ x[c]).rotate_left(12);
    x[a] = x[a].wrapping_add(x[b]);
    x[d] = (x[d] ^ x[a]).rotate_left(8);
    x[c] = x[c].wrapping_add(x[d]);
    x[b] = (x[b] ^ x[c]).rotate_left(7);
}
fn rounds(mut x: [u32; 16], count: usize) -> [u32; 16] {
    for _ in 0..count / 2 {
        for (a, b, c, d) in [
            (0, 4, 8, 12),
            (1, 5, 9, 13),
            (2, 6, 10, 14),
            (3, 7, 11, 15),
            (0, 5, 10, 15),
            (1, 6, 11, 12),
            (2, 7, 8, 13),
            (3, 4, 9, 14),
        ] {
            quarter(&mut x, a, b, c, d);
        }
    }
    x
}
fn state(key: &[u8; 32], tail: &[u8; 16]) -> [u32; 16] {
    let mut bytes = [0; 64];
    bytes[..16].copy_from_slice(b"expand 32-byte k");
    bytes[16..48].copy_from_slice(key);
    bytes[48..].copy_from_slice(tail);
    std::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()))
}
pub fn hchacha(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32] {
    let x = rounds(state(key, nonce), 22);
    let mut out = [0; 32];
    for (j, i) in [0, 1, 2, 3, 12, 13, 14, 15].into_iter().enumerate() {
        out[4 * j..4 * j + 4].copy_from_slice(&x[i].to_le_bytes());
    }
    out
}
pub fn shared(secret: &[u8; 32], public: &[u8; 32]) -> Result<[u8; 32]> {
    let point = x25519_dalek::x25519(*secret, *public);
    if point == [0; 32] {
        bail!("invalid low-order client public key");
    }
    Ok(hchacha(&point, &[0; 16]))
}
pub fn nonce(parts: &[&[u8]]) -> [u8; 24] {
    let mut h = Blake2b::<U24>::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}
fn stream(key: &[u8; 32], nonce: &[u8; 24], length: usize) -> Vec<u8> {
    let sub = hchacha(key, nonce[..16].try_into().unwrap());
    let mut out = Vec::with_capacity(length + 63);
    for counter in 0..length.div_ceil(64) {
        let mut tail = [0; 16];
        tail[..8].copy_from_slice(&(counter as u64).to_le_bytes());
        tail[8..].copy_from_slice(&nonce[16..]);
        let initial = state(&sub, &tail);
        let block = rounds(initial, 18);
        for (x, y) in block.into_iter().zip(initial) {
            out.extend_from_slice(&x.wrapping_add(y).to_le_bytes());
        }
    }
    out.truncate(length);
    out
}
pub fn seal(key: &[u8; 32], nonce: &[u8; 24], plain: &[u8]) -> Vec<u8> {
    let bytes = stream(key, nonce, plain.len() + 32);
    let mut cipher = Vec::with_capacity(plain.len() + 16);
    cipher.extend_from_slice(&[0; 16]);
    cipher.extend(plain.iter().zip(&bytes[32..]).map(|(a, b)| a ^ b));
    let tag = Poly1305::new(&Key::try_from(&bytes[..32]).unwrap()).compute_unpadded(&cipher[16..]);
    cipher[..16].copy_from_slice(&tag);
    cipher
}
pub fn open(key: &[u8; 32], nonce: &[u8; 24], cipher: &[u8]) -> Result<Vec<u8>> {
    if cipher.len() < 16 {
        bail!("ciphertext shorter than authentication tag");
    }
    let bytes = stream(key, nonce, cipher.len() - 16 + 32);
    let tag = Poly1305::new(&Key::try_from(&bytes[..32]).unwrap()).compute_unpadded(&cipher[16..]);
    // Compare all bytes; do not disclose which byte failed.
    if tag
        .iter()
        .zip(&cipher[..16])
        .fold(0u8, |d, (a, b)| d | (a ^ b))
        != 0
    {
        bail!("message authentication failed");
    }
    Ok(cipher[16..]
        .iter()
        .zip(&bytes[32..])
        .map(|(a, b)| a ^ b)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        std::array::from_fn(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
    }
    #[test]
    fn matches_native_x86_box_vector() {
        // Native 0x4d8690, scalar 00..1f, nonce 00..17, original embedded key.
        let secret = std::array::from_fn(|i| i as u8);
        let public = unhex("b5b0544f9dfc356666f934a5a0be24da51453cc1b9e7b845ccdab2339f704341");
        let key = shared(&secret, &public).unwrap();
        assert_eq!(
            key,
            unhex("0fef58ed479b21f2078c75b6213ef1426d56c934b4e320d84aac6de821e0b463")
        );
        let nonce = std::array::from_fn(|i| i as u8);
        let expected = unhex::<27>("d5d94cf7d7a6f380d1a826e6514d16109497d45b492a780e5ef430");
        assert_eq!(seal(&key, &nonce, b"hello world"), expected);
        assert_eq!(open(&key, &nonce, &expected).unwrap(), b"hello world");
        let mut corrupt = expected;
        corrupt[16] ^= 1;
        assert!(open(&key, &nonce, &corrupt).is_err());
        assert!(shared(&secret, &[0; 32]).is_err());
    }
}
