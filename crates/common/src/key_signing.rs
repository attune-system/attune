//! Operator-controlled JWT signing profiles. Private keys never leave this operation.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JwtSigningKey {
    pub private_key_pem: String,
    pub profiles: BTreeMap<String, JwtSigningProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JwtSigningProfile {
    pub issuer: String,
    pub audience: String,
    pub subjects: Vec<String>,
    pub pack_refs: Vec<String>,
    #[serde(default)]
    pub sensor_refs: Vec<String>,
    pub max_ttl_seconds: u32,
}

#[derive(Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

pub struct SignedAssertion {
    pub assertion: String,
    pub expires_at: i64,
}

pub fn sign_approved_jwt(
    key: &JwtSigningKey,
    profile_ref: &str,
    pack_ref: &str,
    subject: &str,
    ttl_seconds: u32,
    now: i64,
) -> Result<SignedAssertion> {
    sign_approved_jwt_for_principal(key, profile_ref, pack_ref, None, subject, ttl_seconds, now)
}

pub fn sign_approved_jwt_for_principal(
    key: &JwtSigningKey,
    profile_ref: &str,
    pack_ref: &str,
    sensor_ref: Option<&str>,
    subject: &str,
    ttl_seconds: u32,
    now: i64,
) -> Result<SignedAssertion> {
    let profile = key
        .profiles
        .get(profile_ref)
        .ok_or_else(|| Error::PermissionDenied("Signing profile is not available".into()))?;
    if !profile.pack_refs.iter().any(|allowed| allowed == pack_ref)
        || sensor_ref.is_some_and(|sensor_ref| {
            !profile
                .sensor_refs
                .iter()
                .any(|allowed| allowed == sensor_ref)
        })
        || !profile.subjects.iter().any(|allowed| allowed == subject)
    {
        return Err(Error::PermissionDenied(
            "Signing profile does not permit this pack and subject".into(),
        ));
    }
    if profile.issuer.is_empty()
        || profile.audience.is_empty()
        || profile.max_ttl_seconds == 0
        || profile.max_ttl_seconds > 300
        || ttl_seconds == 0
        || ttl_seconds > profile.max_ttl_seconds
    {
        return Err(Error::validation(
            "JWT signing profiles require a lifetime between 1 and 300 seconds",
        ));
    }
    let expires_at = now
        .checked_add(i64::from(ttl_seconds))
        .ok_or_else(|| Error::validation("JWT assertion lifetime is invalid"))?;
    let encoding_key = EncodingKey::from_rsa_pem(key.private_key_pem.as_bytes())
        .map_err(|_| Error::invalid_state("Signing key is not a valid RSA private key"))?;
    let pair = aws_lc_rs::signature::RsaKeyPair::from_der(encoding_key.inner())
        .map_err(|_| Error::invalid_state("Signing key is not a supported RSA private key"))?;
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Header::new(Algorithm::RS256))?);
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&AssertionClaims {
        iss: &profile.issuer,
        sub: subject,
        aud: &profile.audience,
        iat: now,
        exp: expires_at,
    })?);
    let message = format!("{header}.{claims}");
    let mut signature = vec![0; pair.public_modulus_len()];
    pair.sign(
        &aws_lc_rs::signature::RSA_PKCS1_SHA256,
        &aws_lc_rs::rand::SystemRandom::new(),
        message.as_bytes(),
        &mut signature,
    )
    .map_err(|_| Error::invalid_state("JWT assertion signing failed"))?;
    let assertion = format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature));
    Ok(SignedAssertion {
        assertion,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::KeyPair;

    fn key() -> JwtSigningKey {
        JwtSigningKey {
            private_key_pem: include_str!("../tests/fixtures/jwt_signing_test_private.pem").into(),
            profiles: BTreeMap::from([(
                "salesforce".into(),
                JwtSigningProfile {
                    issuer: "approved-client".into(),
                    audience: "https://login.salesforce.com".into(),
                    subjects: vec!["sales@example.com".into(), "support@example.com".into()],
                    pack_refs: vec!["sales".into(), "support".into()],
                    sensor_refs: vec!["sales.poll".into()],
                    max_ttl_seconds: 120,
                },
            )]),
        }
    }

    #[test]
    fn assertions_are_signed_and_claims_are_operator_constrained() {
        let key = key();
        for (pack, subject) in [
            ("sales", "sales@example.com"),
            ("support", "support@example.com"),
        ] {
            let signed =
                sign_approved_jwt(&key, "salesforce", pack, subject, 60, 1_800_000_000).unwrap();
            let parts: Vec<_> = signed.assertion.split('.').collect();
            assert_eq!(parts.len(), 3);
            let claims: serde_json::Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
            assert_eq!(claims["iss"], "approved-client");
            assert_eq!(claims["aud"], "https://login.salesforce.com");
            assert_eq!(claims["sub"], subject);
            assert_eq!(claims["exp"], 1_800_000_060_i64);
            assert!(!signed.assertion.contains("PRIVATE KEY"));
            let encoding = EncodingKey::from_rsa_pem(key.private_key_pem.as_bytes()).unwrap();
            let pair = aws_lc_rs::signature::RsaKeyPair::from_der(encoding.inner()).unwrap();
            let public = aws_lc_rs::signature::UnparsedPublicKey::new(
                &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
                pair.public_key().as_ref(),
            );
            public
                .verify(
                    format!("{}.{}", parts[0], parts[1]).as_bytes(),
                    &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
                )
                .unwrap();
        }
    }

    #[test]
    fn unauthorized_packs_subjects_profiles_and_lifetimes_are_rejected() {
        let key = key();
        assert!(
            sign_approved_jwt(&key, "salesforce", "attacker", "sales@example.com", 60, 1).is_err()
        );
        assert!(
            sign_approved_jwt(&key, "salesforce", "sales", "admin@example.com", 60, 1).is_err()
        );
        assert!(sign_approved_jwt(&key, "missing", "sales", "sales@example.com", 60, 1).is_err());
        assert!(
            sign_approved_jwt(&key, "salesforce", "sales", "sales@example.com", 121, 1).is_err()
        );
        assert!(sign_approved_jwt_for_principal(
            &key,
            "salesforce",
            "sales",
            Some("sales.attacker"),
            "sales@example.com",
            60,
            1
        )
        .is_err());
        assert!(sign_approved_jwt_for_principal(
            &key,
            "salesforce",
            "sales",
            Some("sales.poll"),
            "sales@example.com",
            60,
            1
        )
        .is_ok());
    }
}
