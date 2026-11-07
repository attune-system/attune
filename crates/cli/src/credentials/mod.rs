//! Local credential protection against copying the configuration file alone.
//! Identifiers are binding material, not secrets; this is not a hardware vault.
mod identifiers;
pub(crate) mod storage;

use anyhow::{bail, Context, Result};
use attune_common::crypto;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2_10::Sha256;
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub(crate) const UNLOCK_ERROR: &str = "Stored credentials cannot be unlocked; log in again. If system identifiers are unavailable or unstable, explicitly set credential_encryption to false for this profile.";

#[derive(Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokens([redacted])")
    }
}

/// Canonical v1 envelope. No machine identifiers or keys are persisted.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u8,
    pub policy: String,
    pub profile_id: String,
    pub sources: Vec<String>,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize)]
struct Binding<'a> {
    purpose: &'static str,
    version: u8,
    policy: &'a str,
    profile_id: &'a str,
    profile_name: &'a str,
    endpoint: &'a str,
    sources: &'a [String],
    salt: &'a str,
}

pub(crate) fn endpoint(value: &str) -> Result<String> {
    let mut url = url::Url::parse(value).context("Invalid API endpoint")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("API endpoint must be an HTTP(S) URL without credentials, query, or fragment");
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

impl Envelope {
    pub fn seal(tokens: &Tokens, name: &str, url: &str) -> Result<Self> {
        Self::seal_with(tokens, name, url, &identifiers::collect()?)
    }

    fn seal_with(
        tokens: &Tokens,
        name: &str,
        url: &str,
        ids: &BTreeMap<String, String>,
    ) -> Result<Self> {
        identifiers::validate_minimum(ids).context(UNLOCK_ERROR)?;
        let mut selected = ids.clone();
        if selected.contains_key("machine") {
            selected.remove("machine-dmi");
        }
        let ids = &selected;
        let salt = crypto::random_bytes::<32>();
        let id = crypto::random_bytes::<16>();
        let mut envelope = Self {
            version: 1,
            policy: identifiers::policy().into(),
            profile_id: BASE64.encode(id),
            sources: ids.keys().cloned().collect(),
            salt: BASE64.encode(salt),
            nonce: String::new(),
            ciphertext: String::new(),
        };
        let key = envelope.key(ids)?;
        let aad = envelope.aad(name, url)?;
        let plaintext = Zeroizing::new(serde_json::to_vec(tokens)?);
        let encrypted = crypto::encrypt_authenticated(&plaintext, &key, &aad)?;
        envelope.nonce = BASE64.encode(encrypted.nonce);
        envelope.ciphertext = BASE64.encode(encrypted.ciphertext);
        Ok(envelope)
    }

    /// Refreshes keep the enrolled identifiers, profile salt and profile ID.
    /// Only an explicit new login enrolls the current machine again.
    pub fn replace(&self, tokens: &Tokens, name: &str, url: &str) -> Result<Self> {
        let ids = identifiers::collect()?;
        self.open_with(name, url, &ids)
            .map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?;
        let key = self.key(&ids).map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?;
        let plaintext = Zeroizing::new(serde_json::to_vec(tokens)?);
        let mut next = self.clone();
        let encrypted = crypto::encrypt_authenticated(&plaintext, &key, &self.aad(name, url)?)?;
        next.nonce = BASE64.encode(encrypted.nonce);
        next.ciphertext = BASE64.encode(encrypted.ciphertext);
        Ok(next)
    }

    pub fn open(&self, name: &str, url: &str) -> Result<Tokens> {
        // Do not expose source values or cryptographic/library error payloads.
        self.open_with(
            name,
            url,
            &identifiers::collect().map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?,
        )
        .map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))
    }

    fn open_with(&self, name: &str, url: &str, ids: &BTreeMap<String, String>) -> Result<Tokens> {
        let key = self.key(ids)?;
        let nonce: [u8; 12] = BASE64
            .decode(&self.nonce)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid nonce"))?;
        let ciphertext = BASE64.decode(&self.ciphertext)?;
        let plaintext = Zeroizing::new(crypto::decrypt_authenticated(
            &ciphertext,
            &nonce,
            &key,
            &self.aad(name, url)?,
        )?);
        Ok(serde_json::from_slice(&plaintext)?)
    }

    fn key(&self, ids: &BTreeMap<String, String>) -> Result<Zeroizing<[u8; 32]>> {
        if self.version != 1 || self.policy != identifiers::policy() {
            bail!("Unsupported credential format or platform");
        }
        if BASE64.decode(&self.profile_id)?.len() != 16 {
            bail!("Invalid profile ID");
        }
        if self.sources.windows(2).any(|s| s[0] >= s[1]) {
            bail!("Invalid sources");
        }
        let selected: BTreeMap<_, _> = self
            .sources
            .iter()
            .map(|source| {
                ids.get(source)
                    .map(|value| (source.clone(), value.clone()))
                    .context("Missing identifier")
            })
            .collect::<Result<_>>()?;
        identifiers::validate_minimum(&selected)?;
        let material = Zeroizing::new(serde_json::to_vec(&selected)?);
        let salt = BASE64.decode(&self.salt)?;
        if salt.len() != 32 {
            bail!("Invalid salt");
        }
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(Some(&salt), &material)
            .expand(
                format!("attune/cli-credentials/v1/{}", self.profile_id).as_bytes(),
                key.as_mut(),
            )
            .map_err(|_| anyhow::anyhow!("Key derivation failed"))?;
        Ok(key)
    }

    fn aad(&self, name: &str, url: &str) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&Binding {
            purpose: "attune/cli-credentials/v1",
            version: self.version,
            policy: &self.policy,
            profile_id: &self.profile_id,
            profile_name: name,
            endpoint: &endpoint(url)?,
            sources: &self.sources,
            salt: &self.salt,
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids() -> BTreeMap<String, String> {
        [
            ("account", "1000/alice"),
            ("hostname", "workstation"),
            ("machine", "unique-installation"),
            ("cpu", "vendor-model"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }
    #[test]
    fn binding_and_roundtrip() {
        let ids = ids();
        let tokens = Tokens {
            access_token: Some("access-secret".into()),
            refresh_token: Some("refresh-secret".into()),
        };
        let e = Envelope::seal_with(&tokens, "dev", "https://example.com/", &ids).unwrap();
        assert_eq!(
            e.open_with("dev", "https://example.com", &ids).unwrap(),
            tokens
        );
        assert_eq!(*e.key(&ids).unwrap(), *e.clone().key(&ids).unwrap());
        assert!(e.open_with("prod", "https://example.com", &ids).is_err());
        assert!(e.open_with("dev", "https://other.example", &ids).is_err());
        for source in ids.keys() {
            let mut changed = ids.clone();
            changed.insert(source.clone(), "changed".into());
            assert!(e.open_with("dev", "https://example.com", &changed).is_err());
            let mut missing = ids.clone();
            missing.remove(source);
            assert!(e.open_with("dev", "https://example.com", &missing).is_err());
        }
        let mut extended = ids.clone();
        extended.insert("new-optional-source".into(), "ignored".into());
        assert_eq!(
            e.open_with("dev", "https://example.com", &extended)
                .unwrap(),
            tokens
        );
        let next = Envelope::seal_with(&tokens, "dev", "https://example.com", &ids).unwrap();
        assert_ne!(e.nonce, next.nonce);
        assert_ne!(e.salt, next.salt);
        assert!(!serde_json::to_string(&e).unwrap().contains("access-secret"));
        let mut corrupt = e.clone();
        corrupt.ciphertext = BASE64.encode([0u8; 40]);
        assert!(corrupt
            .open_with("dev", "https://example.com", &ids)
            .is_err());
        let mut future = e.clone();
        future.version = 2;
        assert!(future
            .open_with("dev", "https://example.com", &ids)
            .is_err());
    }
}
