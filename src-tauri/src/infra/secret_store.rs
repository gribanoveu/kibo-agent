//! The on-disk format for sealed secrets, and the only way they are written.
//!
//! ## Layout
//!
//! ```text
//! "ATLS" | version | purpose_len: u8 | purpose | nonce (12) | ciphertext+tag
//! \_____________ header, and also the AES-GCM associated data ____________/
//! ```
//!
//! Binding the header — and with it the purpose — as associated data is the
//! whole point of having a format. The obvious layout is a bare
//! `nonce || ciphertext` under one app-wide key, and it makes every sealed
//! file structurally interchangeable: dropping one over another produces a
//! file that decrypts perfectly and hands the wrong secret to whoever asked.
//! A purpose in the AAD turns that into a decryption failure. The version byte
//! is what lets a later format land without guessing.

use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use thiserror::Error;
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"ATLS";
const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

/// What a sealed blob holds. Part of the associated data, so a blob can only
/// ever be opened as the thing it was sealed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretPurpose {
    ProviderApiKey,
    /// An object as it was before the app changed it (`infra::kube_changes`)
    /// — a Secret's backup holds the secrets themselves.
    KubeBackup,
    /// A second purpose, so the suite can prove the one property that needs
    /// two to exist: that a blob restamped as another purpose and opened as
    /// that purpose still fails. Compiled out of release builds — by the time
    /// there is a real second purpose this can go.
    #[cfg(test)]
    Other,
}

impl SecretPurpose {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            SecretPurpose::ProviderApiKey => b"provider-api-key",
            SecretPurpose::KubeBackup => b"kube-backup",
            #[cfg(test)]
            SecretPurpose::Other => b"something-else",
        }
    }
}

#[derive(Debug, Error)]
pub enum SecretStoreError {
    #[error("not a sealed secret")]
    NotSealed,
    #[error("sealed with an unsupported format version: {0}")]
    UnsupportedVersion(u8),
    #[error("sealed as {found}, not as {expected}")]
    WrongPurpose { expected: String, found: String },
    #[error("could not decrypt — wrong key or damaged file")]
    Decrypt,
    #[error("encryption failed: {0}")]
    Encrypt(String),
}

/// Seals `plaintext` under `key` for exactly one purpose.
pub fn seal(
    key: &[u8; 32],
    purpose: SecretPurpose,
    plaintext: &[u8],
) -> Result<Vec<u8>, SecretStoreError> {
    let header = header(purpose);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher = Aes256Gcm::new(key.into());
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &header,
            },
        )
        .map_err(|e| SecretStoreError::Encrypt(e.to_string()))?;

    let mut blob = header;
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Opens a blob, and only as the purpose it was sealed for.
///
/// The plaintext comes back in a `Zeroizing` buffer: a secret that lingers in
/// freed memory is a secret in the next crash dump.
pub fn open(
    key: &[u8; 32],
    purpose: SecretPurpose,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, SecretStoreError> {
    // The header is read out of the file, not rebuilt from what the caller
    // asked for. Rebuilding it looks equivalent and is not: the stored bytes
    // would then be decorative, a tampered header would go unnoticed because
    // nothing authenticates it, and every offset below would come from a guess
    // about the file's shape rather than from the file.
    if blob.len() < MAGIC.len() + 2 || &blob[..MAGIC.len()] != MAGIC {
        return Err(SecretStoreError::NotSealed);
    }
    let version = blob[MAGIC.len()];
    if version != VERSION {
        return Err(SecretStoreError::UnsupportedVersion(version));
    }

    let purpose_len = blob[MAGIC.len() + 1] as usize;
    let header_len = MAGIC.len() + 2 + purpose_len;
    if blob.len() < header_len + NONCE_LEN {
        return Err(SecretStoreError::NotSealed);
    }
    let header = &blob[..header_len];
    let stored_purpose = &blob[MAGIC.len() + 2..header_len];

    // Checked before decrypting so the answer names the mismatch, rather than
    // arriving as an indistinguishable "could not decrypt".
    if stored_purpose != purpose.as_bytes() {
        return Err(SecretStoreError::WrongPurpose {
            expected: String::from_utf8_lossy(purpose.as_bytes()).into_owned(),
            found: String::from_utf8_lossy(stored_purpose).into_owned(),
        });
    }

    let (nonce_bytes, ciphertext) = blob[header_len..].split_at(NONCE_LEN);

    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad: header,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| SecretStoreError::Decrypt)
}

fn header(purpose: SecretPurpose) -> Vec<u8> {
    let purpose = purpose.as_bytes();
    let mut header = Vec::with_capacity(MAGIC.len() + 2 + purpose.len());
    header.extend_from_slice(MAGIC);
    header.push(VERSION);
    header.push(purpose.len() as u8);
    header.extend_from_slice(purpose);
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];
    const OTHER_KEY: [u8; 32] = [9u8; 32];

    #[test]
    fn a_sealed_secret_opens_again() {
        let sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        let opened = open(&KEY, SecretPurpose::ProviderApiKey, &sealed).unwrap();
        assert_eq!(&*opened, b"sk-secret");
    }

    /// The plaintext must not be sitting in the file next to its own header.
    #[test]
    fn the_blob_does_not_contain_the_plaintext() {
        let sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        assert!(
            !sealed.windows(9).any(|w| w == b"sk-secret"),
            "plaintext leaked into the blob"
        );
    }

    /// Nonce reuse under one key is the way to lose AES-GCM entirely, so two
    /// seals of the same bytes must not produce the same file.
    #[test]
    fn sealing_twice_produces_different_blobs() {
        let a = seal(&KEY, SecretPurpose::ProviderApiKey, b"same").unwrap();
        let b = seal(&KEY, SecretPurpose::ProviderApiKey, b"same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn the_wrong_key_cannot_open_it() {
        let sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        assert!(matches!(
            open(&OTHER_KEY, SecretPurpose::ProviderApiKey, &sealed),
            Err(SecretStoreError::Decrypt)
        ));
    }

    /// A tampered byte anywhere — header, nonce or ciphertext — must fail to
    /// authenticate rather than yield altered plaintext.
    #[test]
    fn any_tampering_is_detected() {
        let sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        for index in [1, 6, sealed.len() - 1] {
            let mut damaged = sealed.clone();
            damaged[index] ^= 0x01;
            assert!(
                open(&KEY, SecretPurpose::ProviderApiKey, &damaged).is_err(),
                "a flipped bit at {index} went unnoticed"
            );
        }
    }

    #[test]
    fn something_that_is_not_a_blob_is_refused_as_such() {
        assert!(matches!(
            open(&KEY, SecretPurpose::ProviderApiKey, b"hello"),
            Err(SecretStoreError::NotSealed)
        ));
        assert!(matches!(
            open(&KEY, SecretPurpose::ProviderApiKey, &[0u8; 64]),
            Err(SecretStoreError::NotSealed)
        ));
    }

    /// A future format must announce itself rather than be misread as damage.
    #[test]
    fn an_unknown_version_says_so() {
        let mut sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"x").unwrap();
        sealed[MAGIC.len()] = 99;
        assert!(matches!(
            open(&KEY, SecretPurpose::ProviderApiKey, &sealed),
            Err(SecretStoreError::UnsupportedVersion(99))
        ));
    }

    /// The reason the purpose is in the associated data at all. With a bare
    /// `nonce || ciphertext` under one key, every sealed file is
    /// interchangeable: dropping one over another decrypts perfectly and hands
    /// the wrong secret to whoever asked for the other.
    #[test]
    fn a_blob_sealed_for_another_purpose_is_named_not_merely_refused() {
        let mut sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        // Restamp the stored purpose as something else, as swapping two files
        // would.
        let purpose_at = MAGIC.len() + 2;
        sealed[purpose_at] = b'X';

        match open(&KEY, SecretPurpose::ProviderApiKey, &sealed) {
            Err(SecretStoreError::WrongPurpose { expected, found }) => {
                assert_eq!(expected, "provider-api-key");
                assert!(found.starts_with('X'), "{found}");
            }
            other => panic!("expected a named mismatch, got {other:?}"),
        }
    }

    /// The property the associated data exists for, and the only one that
    /// needs two purposes to express.
    ///
    /// Restamping a blob's stored purpose defeats the plain comparison above —
    /// it now says what the attacker wants, and the caller asks for the same
    /// thing. Only binding the header into the ciphertext catches it. Without
    /// the AAD this swap succeeds and hands out the wrong secret.
    #[test]
    fn a_blob_restamped_as_another_purpose_still_fails_to_open() {
        let sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        let original_header_len = MAGIC.len() + 2 + SecretPurpose::ProviderApiKey.as_bytes().len();

        let mut restamped = header(SecretPurpose::Other);
        restamped.extend_from_slice(&sealed[original_header_len..]);

        assert!(
            matches!(
                open(&KEY, SecretPurpose::Other, &restamped),
                Err(SecretStoreError::Decrypt)
            ),
            "a restamped blob opened as its new purpose — the AAD is not binding"
        );
    }

    /// Even with the stored purpose left alone, a header byte flipped anywhere
    /// must break authentication — the header *is* the associated data.
    #[test]
    fn a_tampered_header_breaks_authentication() {
        let mut sealed = seal(&KEY, SecretPurpose::ProviderApiKey, b"sk-secret").unwrap();
        let purpose_len_at = MAGIC.len() + 1;
        sealed[purpose_len_at] = sealed[purpose_len_at].wrapping_sub(1);

        assert!(open(&KEY, SecretPurpose::ProviderApiKey, &sealed).is_err());
    }
}
