use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};

use crate::error::{AppError, AppResult};

const X25519_BASEPOINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

fn x25519(scalar: [u8; 32], point: [u8; 32]) -> [u8; 32] {
    x25519_dalek::x25519(scalar, point)
}

pub struct IdentityKeys {
    pub server_private_key: Vec<u8>,
    pub server_public_key: Vec<u8>,
}

pub fn generate_identity_keys() -> IdentityKeys {
    let mut x25519_private: [u8; 32] = rand::random();
    x25519_private[0] &= 248;
    x25519_private[31] &= 127;
    x25519_private[31] |= 64;
    let x25519_public = x25519(x25519_private, X25519_BASEPOINT);

    IdentityKeys {
        server_private_key: x25519_private.to_vec(),
        server_public_key: x25519_public.to_vec(),
    }
}

pub fn derive_shared_key(my_private: &[u8], peer_public: &[u8]) -> AppResult<[u8; 32]> {
    let my_private_arr: [u8; 32] = my_private
        .try_into()
        .map_err(|_| AppError::Crypto("invalid private key length".into()))?;
    let peer_public_arr: [u8; 32] = peer_public
        .try_into()
        .map_err(|_| AppError::Crypto("invalid public key length".into()))?;

    Ok(x25519(my_private_arr, peer_public_arr))
}

pub fn encrypt_with_shared_key(data: &[u8], shared_key: &[u8; 32]) -> AppResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(shared_key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, data)
        .map_err(|e| AppError::Crypto(format!("encryption failed: {e}")))?;
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt_with_shared_key(data: &[u8], shared_key: &[u8; 32]) -> AppResult<Vec<u8>> {
    if data.len() < 12 {
        return Err(AppError::Crypto("encrypted data too short".into()));
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(shared_key));
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| AppError::Crypto(format!("decryption failed: {e}")))
}
