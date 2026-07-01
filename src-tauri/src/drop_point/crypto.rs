use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use serde::Deserialize;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::drop_point::manifest::{ManifestError, RecoveredFile, split_payload};

const PROTOCOL_VERSION: u32 = 2;
const KEY_AGREEMENT: &str = "x25519-hkdf-sha256-aesgcm-raw32";
const INFO_METADATA: &[u8] = b"DropPoint/protocol/v2 key=metadata";
const INFO_PAYLOAD: &[u8] = b"DropPoint/protocol/v2 key=payload";
const AAD_METADATA: &[u8] = b"\x02metadata";
const AAD_PAYLOAD: &[u8] = b"\x02payload";
const X25519_KEY_BYTES: usize = 32;
const AES_GCM_NONCE_BYTES: usize = 12;
const AES_GCM_TAG_BYTES: usize = 16;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    protocol_version: u32,
    key_agreement: String,
    sender_ephemeral_public_key: String,
    metadata_nonce: String,
    payload_nonce: String,
    encrypted_metadata: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DropPointCryptoError {
    #[error("envelope JSON is invalid: {0}")]
    EnvelopeJson(#[from] serde_json::Error),
    #[error("unsupported envelope protocol_version {0}")]
    UnsupportedProtocol(u32),
    #[error("unsupported envelope key_agreement {0}")]
    UnsupportedKeyAgreement(String),
    #[error("envelope field {field} is invalid: {reason}")]
    InvalidEnvelopeField { field: &'static str, reason: String },
    #[error("X25519 shared secret is all zero")]
    AllZeroSharedSecret,
    #[error("HKDF expansion failed")]
    Hkdf,
    #[error("AES-GCM decryption failed for {0}")]
    Decrypt(&'static str),
    #[error("decrypted manifest is invalid: {0}")]
    Manifest(#[from] ManifestError),
}

#[must_use]
pub fn generate_recipient_key_pair() -> ([u8; 32], [u8; 32]) {
    let private = StaticSecret::random();
    let public = PublicKey::from(&private);
    (private.to_bytes(), public.to_bytes())
}

#[must_use]
pub fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn decrypt_bundle(
    recipient_private_key: [u8; 32],
    envelope_json: &[u8],
    encrypted_payload: &[u8],
) -> Result<Vec<RecoveredFile>, DropPointCryptoError> {
    let envelope: Envelope = serde_json::from_slice(envelope_json)?;
    validate_envelope_header(&envelope)?;

    let sender_public_key = decode_field_32(
        "sender_ephemeral_public_key",
        &envelope.sender_ephemeral_public_key,
    )?;
    let metadata_nonce = decode_field_12("metadata_nonce", &envelope.metadata_nonce)?;
    let payload_nonce = decode_field_12("payload_nonce", &envelope.payload_nonce)?;
    let encrypted_metadata = decode_field_min(
        "encrypted_metadata",
        &envelope.encrypted_metadata,
        AES_GCM_TAG_BYTES,
    )?;

    let recipient_private = StaticSecret::from(recipient_private_key);
    let recipient_public = PublicKey::from(&recipient_private).to_bytes();
    let sender_public = PublicKey::from(sender_public_key);
    let shared_secret = recipient_private.diffie_hellman(&sender_public);
    if shared_secret.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(DropPointCryptoError::AllZeroSharedSecret);
    }

    let (metadata_key, payload_key) = derive_keys(
        shared_secret.as_bytes(),
        &sender_public_key,
        &recipient_public,
    )?;
    let manifest_json = decrypt_aes_gcm(
        &metadata_key,
        &metadata_nonce,
        AAD_METADATA,
        &encrypted_metadata,
        "metadata",
    )?;
    let payload_plaintext = decrypt_aes_gcm(
        &payload_key,
        &payload_nonce,
        AAD_PAYLOAD,
        encrypted_payload,
        "payload",
    )?;

    Ok(split_payload(&manifest_json, &payload_plaintext)?)
}

fn validate_envelope_header(envelope: &Envelope) -> Result<(), DropPointCryptoError> {
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(DropPointCryptoError::UnsupportedProtocol(
            envelope.protocol_version,
        ));
    }
    if envelope.key_agreement != KEY_AGREEMENT {
        return Err(DropPointCryptoError::UnsupportedKeyAgreement(
            envelope.key_agreement.clone(),
        ));
    }
    Ok(())
}

fn decode_field_32(field: &'static str, value: &str) -> Result<[u8; 32], DropPointCryptoError> {
    let bytes = decode_field_exact(field, value, X25519_KEY_BYTES)?;
    bytes.try_into().map_err(
        |bytes: Vec<u8>| DropPointCryptoError::InvalidEnvelopeField {
            field,
            reason: format!("decoded length = {}, want 32", bytes.len()),
        },
    )
}

fn decode_field_12(field: &'static str, value: &str) -> Result<[u8; 12], DropPointCryptoError> {
    let bytes = decode_field_exact(field, value, AES_GCM_NONCE_BYTES)?;
    bytes.try_into().map_err(
        |bytes: Vec<u8>| DropPointCryptoError::InvalidEnvelopeField {
            field,
            reason: format!("decoded length = {}, want 12", bytes.len()),
        },
    )
}

fn decode_field_exact(
    field: &'static str,
    value: &str,
    len: usize,
) -> Result<Vec<u8>, DropPointCryptoError> {
    let decoded = decode_base64url_field(field, value)?;
    if decoded.len() == len {
        Ok(decoded)
    } else {
        Err(DropPointCryptoError::InvalidEnvelopeField {
            field,
            reason: format!("decoded length = {}, want {len}", decoded.len()),
        })
    }
}

fn decode_field_min(
    field: &'static str,
    value: &str,
    min_len: usize,
) -> Result<Vec<u8>, DropPointCryptoError> {
    let decoded = decode_base64url_field(field, value)?;
    if decoded.len() >= min_len {
        Ok(decoded)
    } else {
        Err(DropPointCryptoError::InvalidEnvelopeField {
            field,
            reason: format!(
                "decoded length = {}, want at least {min_len}",
                decoded.len()
            ),
        })
    }
}

fn decode_base64url_field(
    field: &'static str,
    value: &str,
) -> Result<Vec<u8>, DropPointCryptoError> {
    if value.is_empty() || value.contains('=') {
        return Err(DropPointCryptoError::InvalidEnvelopeField {
            field,
            reason: "base64url value must be non-empty and unpadded".to_string(),
        });
    }
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|e| DropPointCryptoError::InvalidEnvelopeField {
            field,
            reason: e.to_string(),
        })
}

fn derive_keys(
    shared_secret: &[u8; 32],
    sender_public_key: &[u8; 32],
    recipient_public_key: &[u8; 32],
) -> Result<([u8; 32], [u8; 32]), DropPointCryptoError> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(sender_public_key);
    salt[32..].copy_from_slice(recipient_public_key);

    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
    let mut metadata_key = [0u8; 32];
    let mut payload_key = [0u8; 32];
    hkdf.expand(INFO_METADATA, &mut metadata_key)
        .map_err(|_| DropPointCryptoError::Hkdf)?;
    hkdf.expand(INFO_PAYLOAD, &mut payload_key)
        .map_err(|_| DropPointCryptoError::Hkdf)?;
    Ok((metadata_key, payload_key))
}

fn decrypt_aes_gcm(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
    label: &'static str,
) -> Result<Vec<u8>, DropPointCryptoError> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| DropPointCryptoError::Decrypt(label))?;
    let nonce = Nonce::try_from(&nonce[..]).map_err(|_| DropPointCryptoError::Decrypt(label))?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| DropPointCryptoError::Decrypt(label))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    const RECIPIENT_PRIVATE_KEY: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA";
    const SINGLE_ENVELOPE_JSON: &str = concat!(
        r#"{"protocol_version":2,"key_agreement":"x25519-hkdf-sha256-aesgcm-raw32","sender_ephemeral_public_key":"ZLEBsdC-WocEvQePmJUAH8A-jp-VIvGI3RKNmEbUhGY","metadata_nonce":"gYKDhIWGh4iJiouM","payload_nonce":"oaKjpKWmp6ipqqus","encrypted_metadata":"RXCd3ShA60Tza36-2nebwQVpV_NcAFlqtswR1p3V2_CXK9RVNjBXH2SER4pzbkLgtZj8Il4yGrid_PJ1BQatt8XhCygqbzWI5SCXUm-dZwSHv_bZSg6mhLJX6ED"#,
        r#"E8Uuhr0CYIabnfbDEU1swi_mQ6FshM7aLdi-XQzleiuSNyKclXXGJ-5WbPQI"}"#,
    );
    const SINGLE_ENCRYPTED_PAYLOAD: &str = "95kEDw2nrrpQAuknRO8NY2vBLOEvOd2Qjbzwu0aRORaf";
    const MULTI_ENVELOPE_JSON: &str = r#"{"protocol_version":2,"key_agreement":"x25519-hkdf-sha256-aesgcm-raw32","sender_ephemeral_public_key":"ZLEBsdC-WocEvQePmJUAH8A-jp-VIvGI3RKNmEbUhGY","metadata_nonce":"gYKDhIWGh4iJiouM","payload_nonce":"oaKjpKWmp6ipqqus","encrypted_metadata":"RXCd3ShA60Tza36-2nebwQVpV_NcAFlqtswR1p3V2_CXK9RVNjBXH2SER4pzbkLgtZj8Il4yGrid_PJ1BQatt8XhCygqbzWI5SCXUm-dZwSHufaoHQ6rl7pTvh-C3Um041eKIbi3IvPWSVR3wt3E-FszYvzLKhzWXzju4waBbCw4w5KQ2kSyUuMok_Q0WYrUvLSv3oi0BWOb-FUUmM6TzYaCR7s8CeQtq0ntqYg_Bv97Iaw0ytpAW2Uf9UFwpZwMqJ-0i9h37a3-TdU"}"#;
    const MULTI_ENCRYPTED_PAYLOAD: &str = "-ZUaEBanrKFTF8NXKoRgE2SbeAwJTwpaM2YK_eooj2sl";

    fn private_key() -> [u8; 32] {
        decode_test_b64(RECIPIENT_PRIVATE_KEY).try_into().unwrap()
    }

    fn decode_test_b64(value: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(value).unwrap()
    }

    #[test]
    fn decrypts_single_file_vector() {
        let files = decrypt_bundle(
            private_key(),
            SINGLE_ENVELOPE_JSON.as_bytes(),
            &decode_test_b64(SINGLE_ENCRYPTED_PAYLOAD),
        )
        .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "scan-01.txt");
        assert_eq!(files[0].content_type, "text/plain");
        assert_eq!(files[0].data, b"hello drop point\n");
    }

    #[test]
    fn decrypts_multi_file_vector() {
        let files = decrypt_bundle(
            private_key(),
            MULTI_ENVELOPE_JSON.as_bytes(),
            &decode_test_b64(MULTI_ENCRYPTED_PAYLOAD),
        )
        .unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "scan-01.txt");
        assert_eq!(files[0].data, b"first file\n");
        assert_eq!(files[1].filename, "scan-02.bin");
        assert_eq!(files[1].content_type, "application/octet-stream");
        assert_eq!(files[1].data, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn rejects_tampered_payload() {
        let mut payload = decode_test_b64(SINGLE_ENCRYPTED_PAYLOAD);
        payload[0] ^= 1;
        assert!(matches!(
            decrypt_bundle(private_key(), SINGLE_ENVELOPE_JSON.as_bytes(), &payload),
            Err(DropPointCryptoError::Decrypt("payload"))
        ));
    }
}
