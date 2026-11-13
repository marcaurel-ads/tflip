//! The vault: the stored passwords' key, made from the master password and
//! held in memory until the studio exits.

use std::sync::Mutex;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, Generate, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

const NONCE: usize = 12;

/// What the proof seals: opening it shows a master password is the one the
/// vault was made with, without the vault keeping the password.
const PROOF: &[u8] = b"tflip-studio master password";

/// The settings a vault is made with; kept with it, so later ones can be
/// stronger without making the old vaults unreadable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    pub memory: u32,
    pub passes: u32,
    pub lanes: u32,
}

/// OWASP's minimum for Argon2id, which is also the crate's default.
pub const COST: Cost = Cost {
    memory: Params::DEFAULT_M_COST,
    passes: Params::DEFAULT_T_COST,
    lanes: Params::DEFAULT_P_COST,
};

/// What a vault keeps on disk; nothing in it can be checked against a guess
/// without running Argon2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Master {
    pub salt: String,
    pub proof: String,
    pub cost: Cost,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VaultError {
    /// No key yet: the master password has to be given first.
    Locked,
    WrongPassword,
    /// A stored password that does not open: changed by hand, or sealed for
    /// another entry.
    Unreadable,
}

#[derive(Default)]
pub struct Vault {
    key: Mutex<Option<[u8; 32]>>,
}

impl Vault {
    pub fn is_unlocked(&self) -> bool {
        self.key.lock().unwrap().is_some()
    }

    /// A new master for `password`, and the vault unlocked with it.
    pub fn create(&self, password: &str) -> Master {
        let salt: [u8; 16] = random();
        let key = derive(password, &salt, COST);
        let master = Master {
            salt: STANDARD.encode(salt),
            proof: seal_with(&key, PROOF, b""),
            cost: COST,
        };
        *self.key.lock().unwrap() = Some(key);
        master
    }

    pub fn unlock(&self, password: &str, master: &Master) -> Result<(), VaultError> {
        let salt = STANDARD
            .decode(&master.salt)
            .map_err(|_| VaultError::Unreadable)?;
        let key = derive(password, &salt, master.cost);
        match open_with(&key, &master.proof, b"") {
            Ok(proof) if proof == PROOF => {
                *self.key.lock().unwrap() = Some(key);
                Ok(())
            }
            _ => Err(VaultError::WrongPassword),
        }
    }

    /// `aad` binds the sealed text to what it is for, so it cannot be moved
    /// to another entry.
    pub fn seal(&self, plain: &str, aad: &[u8]) -> Result<String, VaultError> {
        let key = self.key.lock().unwrap().ok_or(VaultError::Locked)?;
        Ok(seal_with(&key, plain.as_bytes(), aad))
    }

    pub fn open(&self, sealed: &str, aad: &[u8]) -> Result<String, VaultError> {
        let key = self.key.lock().unwrap().ok_or(VaultError::Locked)?;
        let plain = open_with(&key, sealed, aad)?;
        String::from_utf8(plain).map_err(|_| VaultError::Unreadable)
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).expect("the system's randomness");
    bytes
}

fn derive(password: &str, salt: &[u8], cost: Cost) -> [u8; 32] {
    let params = Params::new(cost.memory, cost.passes, cost.lanes, Some(32))
        .expect("the vault's Argon2 settings");
    let mut key = [0; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("Argon2 with a 16-byte salt");
    key
}

/// The nonce, then the ciphertext and its tag, in base64: text, for the
/// table's character field.
fn seal_with(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> String {
    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce = Nonce::generate();
    let sealed = cipher
        .encrypt(&nonce, Payload { msg: plain, aad })
        .expect("ChaCha20-Poly1305 seals any length the field holds");
    let mut bytes = nonce.to_vec();
    bytes.extend(sealed);
    STANDARD.encode(bytes)
}

fn open_with(key: &[u8; 32], sealed: &str, aad: &[u8]) -> Result<Vec<u8>, VaultError> {
    let bytes = STANDARD
        .decode(sealed)
        .map_err(|_| VaultError::Unreadable)?;
    if bytes.len() < NONCE {
        return Err(VaultError::Unreadable);
    }
    let (nonce, sealed) = bytes.split_at(NONCE);
    let nonce = Nonce::try_from(nonce).map_err(|_| VaultError::Unreadable)?;
    ChaCha20Poly1305::new(key.into())
        .decrypt(&nonce, Payload { msg: sealed, aad })
        .map_err(|_| VaultError::Unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_only_with_the_master_password_and_entry() {
        let vault = Vault::default();
        assert_eq!(vault.seal("pw", b"a"), Err(VaultError::Locked));
        let master = vault.create("master");
        let sealed = vault.seal("adssyspw", b"/data/dict.add\0ADSSYS").unwrap();
        assert_eq!(
            vault.open(&sealed, b"/data/dict.add\0ADSSYS").unwrap(),
            "adssyspw"
        );
        assert_eq!(
            vault.open(&sealed, b"/other/dict.add\0ADSSYS"),
            Err(VaultError::Unreadable)
        );

        // A restart: the key is gone until the master password is given.
        let vault = Vault::default();
        assert_eq!(vault.open(&sealed, b"x"), Err(VaultError::Locked));
        assert_eq!(
            vault.unlock("wrong", &master),
            Err(VaultError::WrongPassword)
        );
        assert!(!vault.is_unlocked());
        vault.unlock("master", &master).unwrap();
        assert_eq!(
            vault.open(&sealed, b"/data/dict.add\0ADSSYS").unwrap(),
            "adssyspw"
        );
    }
}
