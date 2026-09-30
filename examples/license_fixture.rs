//! Offline integration-test issuer. The fixed key is deliberately public test data.
//! Never use this key for a production service or a live-environment build.
use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use evalproof::license::{AUDIENCE, Entitlement, LEASE_VERSION, sign};
use std::path::Path;
fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: license_fixture LEASE_PATH [ORG]")?;
    let key = SigningKey::from_bytes(&[7; 32]);
    let now = evalproof::storage::now()?;
    let lease = Entitlement {
        version: LEASE_VERSION,
        audience: AUDIENCE.into(),
        license_id: "offline-test-only".into(),
        org: std::env::args()
            .nth(2)
            .unwrap_or_else(|| "offline-test".into()),
        issued_at: now,
        expires_at: now + 3600,
        environment: "test".into(),
    };
    evalproof::storage::write_json(Path::new(&path), &sign(&lease, &key)?)?;
    println!("{}", hex::encode(key.verifying_key().to_bytes()));
    Ok(())
}
