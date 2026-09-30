use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const AUDIENCE: &str = "evalproof-ci-v1";
pub const MAX_LEASE_SECONDS: u64 = 7 * 86_400;
pub const LEASE_VERSION: u32 = 2;
/// Public pricing page; release builds can pin a direct checkout link instead.
pub const PRICING_URL: &str = "https://evalproof.dev/#pricing";

pub fn checkout_url() -> &'static str {
    option_env!("EVALPROOF_CHECKOUT_URL").unwrap_or(PRICING_URL)
}
pub fn portal_url() -> &'static str {
    option_env!("EVALPROOF_PORTAL_URL").unwrap_or("https://app.lemonsqueezy.com/my-orders")
}

/// Normalizes an organization name. Each subscription is bound to a limited
/// number of these, so they must be stable, short and unambiguous.
pub fn normalize_org(org: &str) -> Result<String> {
    let org = org.trim().to_ascii_lowercase();
    ensure!(
        (1..=64).contains(&org.len())
            && org
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && org.as_bytes()[0].is_ascii_alphanumeric(),
        "invalid organization name; use your GitHub owner name"
    );
    Ok(org)
}

/// The organization the current environment runs for, if it can be determined.
pub fn current_org() -> Result<Option<String>> {
    [
        "EVALPROOF_ORG",
        "GITHUB_REPOSITORY_OWNER",
        "CI_PROJECT_ROOT_NAMESPACE",
    ]
    .into_iter()
    .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
    .map(|org| normalize_org(&org))
    .transpose()
}

pub async fn bounded_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 65536,
            "licensing response exceeds 64 KiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn validated_license_id(
    data: &serde_json::Value,
    store: u64,
    product: u64,
    variant: u64,
) -> Result<u64> {
    ensure!(
        data["valid"] == true
            && matches!(
                data["license_key"]["status"].as_str(),
                Some("active" | "inactive")
            )
            && data["meta"]["store_id"].as_u64() == Some(store)
            && data["meta"]["product_id"].as_u64() == Some(product)
            && data["meta"]["variant_id"].as_u64() == Some(variant),
        "license is not valid for this product"
    );
    data["license_key"]["id"]
        .as_u64()
        .context("missing license identity")
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entitlement {
    pub version: u32,
    pub audience: String,
    pub license_id: String,
    pub org: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub environment: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEntitlement {
    pub payload: String,
    pub signature: String,
}

pub fn sign(entitlement: &Entitlement, key: &SigningKey) -> Result<SignedEntitlement> {
    let bytes = serde_json::to_vec(entitlement)?;
    Ok(SignedEntitlement {
        payload: STANDARD.encode(&bytes),
        signature: STANDARD.encode(key.sign(&bytes).to_bytes()),
    })
}
pub fn verify(
    signed: &SignedEntitlement,
    key: &VerifyingKey,
    now: u64,
    environment: &str,
    expected_org: Option<&str>,
) -> Result<Entitlement> {
    ensure!(
        signed.payload.len() < 8192 && signed.signature.len() < 256,
        "invalid entitlement size"
    );
    let bytes = STANDARD.decode(&signed.payload)?;
    let signature = Signature::from_slice(&STANDARD.decode(&signed.signature)?)?;
    key.verify_strict(&bytes, &signature)
        .context("invalid entitlement signature")?;
    let lease: Entitlement = serde_json::from_slice(&bytes)?;
    ensure!(
        lease.version == LEASE_VERSION
            && lease.audience == AUDIENCE
            && lease.environment == environment,
        "entitlement belongs to another product or environment"
    );
    ensure!(
        lease.issued_at <= now && lease.expires_at > now,
        "entitlement expired or not yet valid"
    );
    ensure!(
        lease.expires_at.saturating_sub(lease.issued_at) <= MAX_LEASE_SECONDS,
        "entitlement lifetime exceeds seven days"
    );
    if let Some(org) = expected_org {
        ensure!(
            lease.org == org,
            "entitlement belongs to organization {:?}, not {org:?}",
            lease.org
        );
    }
    Ok(lease)
}
pub fn public_key() -> Result<VerifyingKey> {
    // Release artifacts must pin their public key at build time. No runtime override.
    let key=option_env!("EVALPROOF_LICENSE_PUBLIC_KEY").context("this build has no release licensing public key; use diagnose or build with the documented release key")?;
    let bytes: [u8; 32] = hex::decode(key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid licensing public key"))?;
    Ok(VerifyingKey::from_bytes(&bytes)?)
}
pub fn check(path: &Path) -> Result<Entitlement> {
    let signed = if let Ok(value) = std::env::var("EVALPROOF_ENTITLEMENT") {
        ensure!(value.len() < 16_384, "entitlement too large");
        serde_json::from_str(&value)?
    } else {
        crate::storage::read_json(path).context(
            "CI requires a paid entitlement; set EVALPROOF_ENTITLEMENT or run license refresh",
        )?
    };
    verify(
        &signed,
        &public_key()?,
        crate::storage::now()?,
        option_env!("EVALPROOF_LICENSE_ENVIRONMENT").unwrap_or("live"),
        current_org()?.as_deref(),
    )
}

pub async fn refresh(path: &Path) -> Result<Entitlement> {
    let key = std::env::var("EVALPROOF_LICENSE_KEY")
        .context("set EVALPROOF_LICENSE_KEY from your purchase receipt")?;
    ensure!(
        !key.is_empty() && key.len() <= 512,
        "invalid license key length"
    );
    let org = current_org()?.context(
        "set EVALPROOF_ORG to your GitHub owner name; GitHub Actions sets GITHUB_REPOSITORY_OWNER automatically",
    )?;
    let endpoint = option_env!("EVALPROOF_ENTITLEMENT_URL")
        .context("this build has no entitlement service URL")?;
    ensure!(
        endpoint.starts_with("https://"),
        "entitlement service requires HTTPS"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .post(endpoint)
        .json(&serde_json::json!({"license_key":key,"org":org}))
        .send()
        .await
        .context("entitlement service unavailable")?;
    if !response.status().is_success() {
        let status = response.status();
        let reason = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "entitlement refresh was rejected ({status}: {}); existing offline entitlement is unchanged",
            reason.chars().take(200).collect::<String>()
        );
    }
    let signed: SignedEntitlement = bounded_json(response).await?;
    let lease = verify(
        &signed,
        &public_key()?,
        crate::storage::now()?,
        option_env!("EVALPROOF_LICENSE_ENVIRONMENT").unwrap_or("live"),
        Some(&org),
    )?;
    crate::storage::write_json(path, &signed)?;
    Ok(lease)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn licenses_are_bound_to_the_purchased_product() -> Result<()> {
        let mut data = serde_json::json!({"valid":true,"license_key":{"id":9,"status":"inactive"},"meta":{"store_id":1,"product_id":2,"variant_id":3}});
        assert_eq!(validated_license_id(&data, 1, 2, 3)?, 9);
        assert!(validated_license_id(&data, 1, 2, 4).is_err());
        data["license_key"]["status"] = serde_json::json!("disabled");
        assert!(validated_license_id(&data, 1, 2, 3).is_err());
        Ok(())
    }
    #[test]
    fn signature_expiration_and_environment_are_enforced() -> Result<()> {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut lease = Entitlement {
            version: LEASE_VERSION,
            audience: AUDIENCE.into(),
            license_id: "test".into(),
            org: "acme".into(),
            issued_at: 100,
            expires_at: 1000,
            environment: "test".into(),
        };
        let signed = sign(&lease, &key)?;
        let public = key.verifying_key();
        assert!(verify(&signed, &public, 101, "test", None).is_ok());
        assert!(verify(&signed, &public, 1000, "test", None).is_err());
        assert!(verify(&signed, &public, 101, "live", None).is_err());
        let mut wrong = sign(&lease, &key)?;
        wrong.payload = STANDARD.encode(b"{}");
        assert!(verify(&wrong, &public, 101, "test", None).is_err());
        lease.version = 1;
        assert!(verify(&sign(&lease, &key)?, &public, 101, "test", None).is_err());
        Ok(())
    }
    #[test]
    fn leases_are_bound_to_one_organization() -> Result<()> {
        let key = SigningKey::from_bytes(&[7; 32]);
        let lease = Entitlement {
            version: LEASE_VERSION,
            audience: AUDIENCE.into(),
            license_id: "test".into(),
            org: "acme".into(),
            issued_at: 100,
            expires_at: 1000,
            environment: "test".into(),
        };
        let signed = sign(&lease, &key)?;
        let public = key.verifying_key();
        assert!(verify(&signed, &public, 101, "test", Some("acme")).is_ok());
        assert!(verify(&signed, &public, 101, "test", Some("other")).is_err());
        Ok(())
    }
    #[test]
    fn organization_names_are_normalized() {
        assert_eq!(normalize_org(" Acme-Labs ").unwrap(), "acme-labs");
        assert!(normalize_org("").is_err());
        assert!(normalize_org("-acme").is_err());
        assert!(normalize_org("acme/labs").is_err());
        assert!(normalize_org(&"a".repeat(65)).is_err());
    }
}
