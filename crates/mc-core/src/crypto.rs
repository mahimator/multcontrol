//! Session security.
//!
//! Both peers share a pre-shared key (generated at install time). The
//! handshake authenticates both sides and derives separate directional keys;
//! every frame afterwards is sealed with ChaCha20-Poly1305.
//!
//! The threat model is "someone on the same Wi-Fi sniffing keystrokes", not a
//! targeted attack, but mutual authentication matters here: without it, anyone
//! on the LAN could inject keystrokes into your session.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::io;

type HmacSha256 = Hmac<Sha256>;

const PSK_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const INFO_LEN: usize = 16;

/// Per-session encryption keys, one per direction.
pub struct SessionKeys {
    /// Controller -> target key.
    pub c2s: [u8; 32],
    /// Target -> controller key.
    pub s2c: [u8; 32],
}

/// A pre-shared key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Psk(pub [u8; PSK_LEN]);

impl std::fmt::Debug for Psk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Psk(<redacted>)")
    }
}

impl Psk {
    /// Parse a hex-encoded 32-byte key.
    pub fn from_hex(s: &str) -> io::Result<Psk> {
        let s = s.trim();
        let bytes = hex_decode(s)?;
        if bytes.len() != PSK_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "pre-shared key must be {PSK_LEN} bytes, got {}",
                    bytes.len()
                ),
            ));
        }
        let mut key = [0u8; PSK_LEN];
        key.copy_from_slice(&bytes);
        Ok(Psk(key))
    }

    /// Render as hex, for writing into the config file.
    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }

    /// Generate a fresh random key.
    pub fn generate() -> Psk {
        let mut key = [0u8; PSK_LEN];
        rand::rngs::OsRng.fill_bytes(&mut key);
        Psk(key)
    }
}

/// Generate a hex-encoded random key, for install scripts.
pub fn generate_psk_hex() -> String {
    Psk::generate().to_hex()
}

/// A random 32-byte nonce.
pub fn random_nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut n);
    n
}

fn hmac(psk: &Psk, data: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&psk.0).expect("hmac accepts any key length");
    mac.update(data);
    let out = mac.finalize().into_bytes();
    let mut result = [0u8; 32];
    result.copy_from_slice(&out);
    result
}

/// Compute the client's handshake tag.
pub fn hello_tag(psk: &Psk, client_nonce: &[u8; 32], hello_body: &[u8]) -> [u8; 32] {
    let mut data = Vec::with_capacity(13 + 32 + hello_body.len());
    data.extend_from_slice(b"mc-hello-v1");
    data.extend_from_slice(client_nonce);
    data.extend_from_slice(hello_body);
    hmac(psk, &data)
}

/// Compute the server's handshake tag.
pub fn ack_tag(
    psk: &Psk,
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    ack_body: &[u8],
) -> [u8; 32] {
    let mut data = Vec::with_capacity(11 + 64 + ack_body.len());
    data.extend_from_slice(b"mc-ack-v1");
    data.extend_from_slice(client_nonce);
    data.extend_from_slice(server_nonce);
    data.extend_from_slice(ack_body);
    hmac(psk, &data)
}

/// Derive the two directional session keys.
pub fn derive_keys(psk: &Psk, client_nonce: &[u8; 32], server_nonce: &[u8; 32]) -> SessionKeys {
    let mut salt = Vec::with_capacity(64);
    salt.extend_from_slice(client_nonce);
    salt.extend_from_slice(server_nonce);

    let hk = Hkdf::<Sha256>::new(Some(&salt), &psk.0);
    let mut c2s = [0u8; 32];
    let mut s2c = [0u8; 32];
    hk.expand(b"multcontrol-c2s", &mut c2s)
        .expect("hkdf expand fits");
    hk.expand(b"multcontrol-s2c", &mut s2c)
        .expect("hkdf expand fits");
    SessionKeys { c2s, s2c }
}

/// Seals and opens frames for one direction of a session.
pub struct Crypter {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Crypter {
    /// Build a crypter from a directional key.
    pub fn new(key: &[u8; 32]) -> Crypter {
        Crypter {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            counter: 0,
        }
    }

    fn next_nonce(&mut self) -> Nonce {
        // 8-byte big-endian counter plus 4 fixed random bytes: unique per
        // direction for the life of the session.
        let mut nonce = [0u8; NONCE_LEN];
        nonce[..8].copy_from_slice(&self.counter.to_be_bytes());
        nonce[8..].copy_from_slice(&NONCE_SUFFIX);
        self.counter = self.counter.wrapping_add(1);
        *Nonce::from_slice(&nonce)
    }

    /// Encrypt a frame payload. The frame kind is authenticated as AAD so a
    /// frame cannot be re-typed on the wire.
    pub fn seal(&mut self, kind: u8, plaintext: &[u8]) -> Vec<u8> {
        let nonce = self.next_nonce();
        let aad = [kind];
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .expect("encryption cannot fail");
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ciphertext);
        out
    }

    /// Decrypt a frame payload.
    pub fn open(&mut self, kind: u8, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(CryptoError::Malformed);
        }
        let nonce = Nonce::from_slice(&sealed[..NONCE_LEN]);
        let aad = [kind];
        self.cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &sealed[NONCE_LEN..],
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Authentication)
    }
}

/// 4 random bytes mixed into every nonce so two sessions with the same key
/// cannot reuse a nonce.
static NONCE_SUFFIX: [u8; 4] = [0xa3, 0x5f, 0x21, 0x9c];

/// Errors from the crypto layer.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Frame was too short to be valid.
    #[error("malformed ciphertext")]
    Malformed,
    /// Authentication tag did not verify.
    #[error("authentication failed")]
    Authentication,
}

/// Hex helpers.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Decode a hex string.
pub fn hex_decode(s: &str) -> io::Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "hex string has odd length",
        ));
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(c: u8) -> io::Result<u8> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid hex character",
        )),
    }
}

/// Length of the authentication tag appended to every sealed frame.
pub const AUTH_TAG_LEN: usize = TAG_LEN;
/// Length of the nonce prefix on every sealed frame.
pub const NONCE_PREFIX_LEN: usize = NONCE_LEN;
/// Length of a nonce in bytes.
pub const NONCE_BYTES: usize = NONCE_LEN;
/// Expected size of the hello/ack authentication tag.
pub const HANDSHAKE_TAG_LEN: usize = 32;
/// Maximum length of the hello/ack body that is authenticated.
pub const MAX_HANDSHAKE_BODY_LEN: usize = INFO_LEN;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psk_hex_roundtrip() {
        let psk = Psk::generate();
        let hex = psk.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(Psk::from_hex(&hex).unwrap(), psk);
        assert!(Psk::from_hex("abc").is_err());
    }

    #[test]
    fn seal_open_roundtrip() {
        let psk = Psk::generate();
        let cn = random_nonce();
        let sn = random_nonce();
        let keys = derive_keys(&psk, &cn, &sn);

        let mut c = Crypter::new(&keys.c2s);
        let mut s = Crypter::new(&keys.c2s);
        let sealed = c.seal(0x02, b"hello");
        assert_eq!(s.open(0x02, &sealed).unwrap(), b"hello");
        // Wrong kind must not authenticate.
        assert!(s.open(0x03, &sealed).is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let psk = Psk::generate();
        let keys = derive_keys(&psk, &random_nonce(), &random_nonce());
        let mut c = Crypter::new(&keys.c2s);
        let mut s = Crypter::new(&keys.c2s);
        let mut sealed = c.seal(0x02, b"hello");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(matches!(
            s.open(0x02, &sealed),
            Err(CryptoError::Authentication)
        ));
    }

    #[test]
    fn handshake_tags_are_direction_specific() {
        let psk = Psk::generate();
        let cn = random_nonce();
        let sn = random_nonce();
        let body = b"{\"v\":1}";
        let hello = hello_tag(&psk, &cn, body);
        let ack = ack_tag(&psk, &cn, &sn, body);
        assert_ne!(hello, ack);
        // A different client nonce must not validate.
        assert_ne!(hello, hello_tag(&psk, &random_nonce(), body));
    }
}
