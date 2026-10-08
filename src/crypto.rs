use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
pub fn token() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
pub fn hash(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}
pub fn password(s: &str) -> Result<String> {
    if s.len() < 12 || s.len() > 256 {
        return Err(anyhow!("Password must be 12–256 bytes"));
    }
    Ok(Argon2::default()
        .hash_password(s.as_bytes(), &SaltString::generate(&mut OsRng))
        .map_err(|_| anyhow!("Password hashing failed"))?
        .to_string())
}
pub fn verify(s: &str, h: &str) -> bool {
    PasswordHash::new(h)
        .map(|h| Argon2::default().verify_password(s.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}
pub fn seal(key: &[u8; 32], s: &str) -> Result<String> {
    let mut n = [0; 12];
    OsRng.fill_bytes(&mut n);
    let c = Aes256Gcm::new_from_slice(key)
        .unwrap()
        .encrypt(Nonce::from_slice(&n), s.as_bytes())
        .map_err(|_| anyhow!("Encryption failed"))?;
    Ok(format!("{}.{}", hex::encode(n), hex::encode(c)))
}
pub fn open(key: &[u8; 32], s: &str) -> Result<String> {
    let (n, c) = s
        .split_once('.')
        .ok_or_else(|| anyhow!("Invalid ciphertext"))?;
    let n = hex::decode(n)?;
    if n.len() != 12 {
        return Err(anyhow!("Invalid nonce"));
    }
    let p = Aes256Gcm::new_from_slice(key)
        .unwrap()
        .decrypt(Nonce::from_slice(&n), hex::decode(c)?.as_ref())
        .map_err(|_| anyhow!("Decryption failed"))?;
    Ok(String::from_utf8(p)?)
}
pub fn equal(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encryption_roundtrip() {
        let k = [7; 32];
        let c = seal(&k, "secret").unwrap();
        assert_eq!(open(&k, &c).unwrap(), "secret");
        assert!(open(&[8; 32], &c).is_err())
    }
}
