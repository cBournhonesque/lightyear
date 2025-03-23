use no_std_io2::io as io;

use chacha20poly1305::{
    aead::{rand_core::RngCore, OsRng},
    AeadInPlace, ChaCha20Poly1305, KeyInit, Tag, XChaCha20Poly1305, XNonce,
};
use crate::serialize::writer::{WriteInteger};
use super::{MAC_BYTES, PRIVATE_KEY_BYTES};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("buffer size mismatch")]
    BufferSizeMismatch,
    #[cfg(feature = "std")]
    #[error("failed to encrypt: {0}")]
    Failed(#[from] chacha20poly1305::aead::Error),
    #[error("failed to generate key: {0}")]
    GenerateKey(chacha20poly1305::aead::rand_core::Error),
}

/// A 32-byte array, used as a key for encrypting and decrypting packets and connect tokens.
pub type Key = [u8; PRIVATE_KEY_BYTES];
pub type Result<T> = core::result::Result<T, Error>;

/// Generates a random key for encrypting and decrypting packets and connect tokens.
///
/// Panics if the underlying RNG fails (highly unlikely). <br>
/// For a non-panicking version, see [`try_generate_key`](fn.try_generate_key.html).
///
/// # Example
/// ```
/// use crate::lightyear::connection::netcode::generate_key;
///
/// let key = generate_key();
/// assert_eq!(key.len(), 32);
/// ```
pub fn generate_key() -> Key {
    let mut key: Key = [0; PRIVATE_KEY_BYTES];
    OsRng.fill_bytes(&mut key);
    key
}

/// The fallible version of [`generate_key`](fn.generate_key.html).
///
/// Returns an error if the underlying RNG fails (highly unlikely).
///
/// # Example
/// ```
/// use crate::lightyear::connection::netcode::try_generate_key;
///
/// let key = try_generate_key().unwrap();
/// assert_eq!(key.len(), 32);
/// ```
pub fn try_generate_key() -> Result<Key> {
    let mut key: Key = [0; PRIVATE_KEY_BYTES];
    OsRng.try_fill_bytes(&mut key).map_err(Error::GenerateKey)?;
    Ok(key)
}

pub fn chacha_encrypt(
    buf: &mut [u8],
    associated_data: Option<&[u8]>,
    nonce: u64,
    key: &Key,
) -> Result<()> {
    let size = buf.len();
    if size < MAC_BYTES {
        // Should have 16 bytes of extra space for the MAC
        return Err(Error::BufferSizeMismatch);
    }
    let mut final_nonce = [0; 12];
    io::Cursor::new(&mut final_nonce[4..]).write_u64(nonce)?;
    let mac = ChaCha20Poly1305::new(key.into()).encrypt_in_place_detached(
        &final_nonce.into(),
        associated_data.unwrap_or_default(),
        &mut buf[..size - MAC_BYTES],
    );
    #[cfg(feature = "std")]
    let mac = mac?;
    #[cfg(not(feature = "std"))]
    let mac = mac.expect("could not encrypt ConnectToken");
    buf[size - MAC_BYTES..].copy_from_slice(mac.as_ref());
    Ok(())
}

pub fn chacha_decrypt(
    buf: &mut [u8],
    associated_data: Option<&[u8]>,
    nonce: u64,
    key: &Key,
) -> Result<()> {
    if buf.len() < MAC_BYTES {
        // Should already include the MAC
        return Err(Error::BufferSizeMismatch);
    }
    let mut final_nonce = [0; 12];
    io::Cursor::new(&mut final_nonce[4..]).write_u64(nonce)?;
    let (buf, mac) = buf.split_at_mut(buf.len() - MAC_BYTES);
    let res = ChaCha20Poly1305::new(key.into()).decrypt_in_place_detached(
        &final_nonce.into(),
        associated_data.unwrap_or_default(),
        buf,
        Tag::from_slice(mac),
    );
    #[cfg(feature = "std")]
    res?;
    #[cfg(not(feature = "std"))]
    res.expect("could not decrypt ConnectToken");
    Ok(())
}

pub fn xchacha_encrypt(
    buf: &mut [u8],
    associated_data: Option<&[u8]>,
    nonce: XNonce,
    key: &Key,
) -> Result<()> {
    let size = buf.len();
    if size < MAC_BYTES {
        // Should have 16 bytes of extra space for the MAC
        return Err(Error::BufferSizeMismatch);
    }
    let mac = XChaCha20Poly1305::new(key.into()).encrypt_in_place_detached(
        &nonce,
        associated_data.unwrap_or_default(),
        &mut buf[..size - MAC_BYTES],
    );
    #[cfg(feature = "std")]
    let mac = mac?;
    #[cfg(not(feature = "std"))]
    let mac = mac.expect("could not encrypt ConnectToken");
    buf[size - MAC_BYTES..].copy_from_slice(mac.as_ref());
    Ok(())
}

pub fn xchacha_decrypt(
    buf: &mut [u8],
    associated_data: Option<&[u8]>,
    nonce: XNonce,
    key: &Key,
) -> Result<()> {
    if buf.len() < MAC_BYTES {
        // Should already include the MAC
        return Err(Error::BufferSizeMismatch);
    }
    let (buf, mac) = buf.split_at_mut(buf.len() - MAC_BYTES);
    let res = XChaCha20Poly1305::new(key.into()).decrypt_in_place_detached(
        &nonce,
        associated_data.unwrap_or_default(),
        buf,
        Tag::from_slice(mac),
    );
    #[cfg(feature = "std")]
    res?;
    #[cfg(not(feature = "std"))]
    res.expect("could not decrypt ConnectToken");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buf_too_small() {
        let mut buf = [0; 0];
        let nonce = 0;
        let key = generate_key();
        let result = chacha_encrypt(&mut buf, None, nonce, &key);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_decrypt_zero_sized_buf() {
        let mut buf = [0u8; MAC_BYTES]; // 16 bytes is the minimum size, which our actual buf is empty
        let nonce = 0;
        let key = generate_key();
        chacha_encrypt(&mut buf, None, nonce, &key).unwrap();

        // The buf should have been modified
        assert_ne!(buf, [0u8; MAC_BYTES]);

        chacha_decrypt(&mut buf, None, nonce, &key).unwrap();
    }
}
