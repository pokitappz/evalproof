#![forbid(unsafe_code)]
//! Stateless entitlement service. Lemon Squeezy is the system of record: each
//! organization that uses a subscription is one license key instance, so the
//! product's activation limit caps how many organizations share a key.
//! API reference: https://docs.lemonsqueezy.com/api/license-api and
//! https://docs.lemonsqueezy.com/api/license-key-instances
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post},
};
use ed25519_dalek::SigningKey;
use evalproof::license::{
    AUDIENCE, Entitlement, LEASE_VERSION, MAX_LEASE_SECONDS, SignedEntitlement, normalize_org,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const GLOBAL_LIMIT_PER_MINUTE: u32 = 50;
const KEY_LIMIT_PER_MINUTE: u32 = 6;

type Rejection = (StatusCode, &'static str);

#[derive(Default)]
struct Limits {
    window: Option<Instant>,
    total: u32,
    per_key: HashMap<[u8; 32], u32>,
}

struct App {
    key: SigningKey,
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    store: u64,
    product: u64,
    variant: u64,
    environment: String,
    limit: Mutex<Limits>,
    // Serializes instance lookup and activation so concurrent CI jobs for a new
    // organization cannot consume two activations. The service runs as one process.
    activation: Mutex<()>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    license_key: String,
    org: String,
}

async fn rate_limit(app: &App, license_key: &str) -> Result<(), Rejection> {
    let digest: [u8; 32] = Sha256::digest(license_key.as_bytes()).into();
    let mut limit = app.limit.lock().await;
    if limit
        .window
        .is_none_or(|w| w.elapsed() >= Duration::from_secs(60))
    {
        *limit = Limits {
            window: Some(Instant::now()),
            ..Limits::default()
        };
    }
    let used = limit.per_key.get(&digest).copied().unwrap_or(0);
    if limit.total >= GLOBAL_LIMIT_PER_MINUTE || used >= KEY_LIMIT_PER_MINUTE {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "Retry later; cached entitlements remain valid",
        ));
    }
    limit.total += 1;
    limit.per_key.insert(digest, used + 1);
    Ok(())
}

async fn license_call(app: &App, path: &str, form: &[(&str, &str)]) -> Result<Value, Rejection> {
    const UNAVAILABLE: Rejection = (
        StatusCode::SERVICE_UNAVAILABLE,
        "License provider unavailable",
    );
    let response = app
        .client
        .post(format!("{}{path}", app.api_base))
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    // Invalid keys are reported with a JSON body and a 4xx status; only 5xx is an outage.
    if response.status().is_server_error() {
        return Err(UNAVAILABLE);
    }
    evalproof::license::bounded_json(response)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Invalid provider response"))
}

async fn instance_exists(app: &App, license_id: u64, org: &str) -> Result<bool, Rejection> {
    const UNAVAILABLE: Rejection = (
        StatusCode::SERVICE_UNAVAILABLE,
        "License provider unavailable",
    );
    let response = app
        .client
        .get(format!("{}/v1/license-key-instances", app.api_base))
        .query(&[
            ("filter[license_key_id]", license_id.to_string()),
            ("page[size]", "100".into()),
        ])
        .header("Accept", "application/vnd.api+json")
        .bearer_auth(&app.api_key)
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    if !response.status().is_success() {
        return Err(UNAVAILABLE);
    }
    let data: Value = evalproof::license::bounded_json(response)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Invalid provider response"))?;
    Ok(data["data"].as_array().is_some_and(|instances| {
        instances.iter().any(|i| {
            i["attributes"]["name"]
                .as_str()
                .and_then(|name| normalize_org(name).ok())
                .is_some_and(|name| name == org)
        })
    }))
}

async fn issue(
    State(app): State<Arc<App>>,
    Json(request): Json<Request>,
) -> Result<Json<SignedEntitlement>, Rejection> {
    if request.license_key.is_empty() || request.license_key.len() > 512 {
        return Err((StatusCode::BAD_REQUEST, "Invalid license key"));
    }
    let org = normalize_org(&request.org)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid organization name"))?;
    rate_limit(&app, &request.license_key).await?;
    let data = license_call(
        &app,
        "/v1/licenses/validate",
        &[("license_key", &request.license_key)],
    )
    .await?;
    let id = evalproof::license::validated_license_id(&data, app.store, app.product, app.variant)
        .map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            "License is not valid for this product",
        )
    })?;
    {
        let _guard = app.activation.lock().await;
        if !instance_exists(&app, id, &org).await? {
            let activated = license_call(
                &app,
                "/v1/licenses/activate",
                &[
                    ("license_key", &request.license_key),
                    ("instance_name", &org),
                ],
            )
            .await?;
            if activated["activated"] != true {
                return Err((
                    StatusCode::FORBIDDEN,
                    "This subscription is already used by its maximum number of organizations; add a seat or deactivate one in the billing portal",
                ));
            }
        }
    }
    // Test and live products have separate allowlisted IDs and separately signed builds.
    let now = evalproof::storage::now()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Clock unavailable"))?;
    let lease = Entitlement {
        version: LEASE_VERSION,
        audience: AUDIENCE.into(),
        license_id: id.to_string(),
        org,
        issued_at: now,
        expires_at: now + MAX_LEASE_SECONDS,
        environment: app.environment.clone(),
    };
    evalproof::license::sign(&lease, &app.key)
        .map(Json)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Unable to issue entitlement",
            )
        })
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/entitlements", post(issue))
        .layer(DefaultBodyLimit::max(2048))
        .with_state(app)
}

#[tokio::main]
async fn main() -> Result<()> {
    let secret = std::env::var("EVALPROOF_SIGNING_KEY_HEX").context("missing signing key")?;
    let bytes: [u8; 32] = hex::decode(secret)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must be 32 bytes"))?;
    let environment = std::env::var("EVALPROOF_LICENSE_ENVIRONMENT").unwrap_or("test".into());
    ensure!(
        matches!(environment.as_str(), "test" | "live"),
        "invalid licensing environment"
    );
    let app = Arc::new(App {
        key: SigningKey::from_bytes(&bytes),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        api_base: "https://api.lemonsqueezy.com".into(),
        api_key: std::env::var("LEMONSQUEEZY_API_KEY")
            .context("missing LEMONSQUEEZY_API_KEY (read access to license key instances)")?,
        store: std::env::var("LEMONSQUEEZY_STORE_ID")?.parse()?,
        product: std::env::var("LEMONSQUEEZY_PRODUCT_ID")?.parse()?,
        variant: std::env::var("LEMONSQUEEZY_VARIANT_ID")?.parse()?,
        environment,
        limit: Mutex::new(Limits::default()),
        activation: Mutex::new(()),
    });
    let bind = std::env::var("BIND_ADDRESS").unwrap_or("127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            if tokio::signal::ctrl_c().await.is_err() {
                eprintln!("Shutdown signal unavailable");
            }
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Form, extract::Query};
    use evalproof::license::verify;

    /// Minimal Lemon Squeezy stand-in holding instance names for key "good" (id 9).
    #[derive(Default)]
    struct Mock {
        instances: std::sync::Mutex<Vec<String>>,
        activation_limit: usize,
    }

    fn meta() -> Value {
        serde_json::json!({"store_id":1,"product_id":2,"variant_id":3})
    }

    async fn mock_provider(
        activation_limit: usize,
        existing: &[&str],
    ) -> Result<(String, Arc<Mock>)> {
        let mock = Arc::new(Mock {
            instances: std::sync::Mutex::new(existing.iter().map(|s| s.to_string()).collect()),
            activation_limit,
        });
        let router = Router::new()
            .route(
                "/v1/licenses/validate",
                post(|Form(form): Form<HashMap<String, String>>| async move {
                    match form["license_key"].as_str() {
                        "good" => (StatusCode::OK, Json(serde_json::json!({"valid":true,"license_key":{"id":9,"status":"active"},"meta":meta()}))),
                        "disabled" => (StatusCode::OK, Json(serde_json::json!({"valid":false,"license_key":{"id":10,"status":"disabled"},"meta":meta()}))),
                        _ => (StatusCode::NOT_FOUND, Json(serde_json::json!({"valid":false,"error":"license_key not found."}))),
                    }
                }),
            )
            .route(
                "/v1/license-key-instances",
                get(|State(mock): State<Arc<Mock>>, Query(query): Query<HashMap<String, String>>| async move {
                    assert_eq!(query["filter[license_key_id]"], "9");
                    let data: Vec<Value> = mock.instances.lock().unwrap().iter().map(|n| serde_json::json!({"attributes":{"name":n,"license_key_id":9}})).collect();
                    Json(serde_json::json!({"data":data}))
                }),
            )
            .route(
                "/v1/licenses/activate",
                post(|State(mock): State<Arc<Mock>>, Form(form): Form<HashMap<String, String>>| async move {
                    let mut instances = mock.instances.lock().unwrap();
                    if instances.len() >= mock.activation_limit {
                        return Json(serde_json::json!({"activated":false,"error":"This license key has reached the activation limit.","meta":meta()}));
                    }
                    instances.push(form["instance_name"].clone());
                    Json(serde_json::json!({"activated":true,"instance":{"id":"x","name":form["instance_name"]},"meta":meta()}))
                }),
            )
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        tokio::spawn(async move { axum::serve(listener, router).await });
        Ok((format!("http://{address}"), mock))
    }

    fn app(api_base: String) -> Arc<App> {
        Arc::new(App {
            key: SigningKey::from_bytes(&[7; 32]),
            client: reqwest::Client::new(),
            api_base,
            api_key: "test".into(),
            store: 1,
            product: 2,
            variant: 3,
            environment: "test".into(),
            limit: Mutex::new(Limits::default()),
            activation: Mutex::new(()),
        })
    }

    async fn request(app: &Arc<App>, key: &str, org: &str) -> Result<Entitlement, StatusCode> {
        let signed = issue(
            State(app.clone()),
            Json(Request {
                license_key: key.into(),
                org: org.into(),
            }),
        )
        .await
        .map_err(|(status, _)| status)?
        .0;
        Ok(verify(
            &signed,
            &app.key.verifying_key(),
            evalproof::storage::now().unwrap(),
            "test",
            None,
        )
        .unwrap())
    }

    #[tokio::test]
    async fn organizations_are_bound_without_reusing_activations() -> Result<()> {
        let (base, mock) = mock_provider(1, &[]).await?;
        let app = app(base);
        let lease = request(&app, "good", "Acme").await.unwrap();
        assert_eq!(lease.org, "acme");
        assert_eq!(lease.version, LEASE_VERSION);
        // Repeated CI refreshes reuse the existing instance.
        request(&app, "good", "acme").await.unwrap();
        assert_eq!(*mock.instances.lock().unwrap(), vec!["acme".to_owned()]);
        // A second organization exceeds the one-seat activation limit.
        assert_eq!(
            request(&app, "good", "other").await.unwrap_err(),
            StatusCode::FORBIDDEN
        );
        Ok(())
    }

    #[tokio::test]
    async fn existing_instances_are_reused() -> Result<()> {
        let (base, mock) = mock_provider(1, &["acme"]).await?;
        request(&app(base), "good", "acme").await.unwrap();
        assert_eq!(mock.instances.lock().unwrap().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_requests_and_keys_are_refused() -> Result<()> {
        let (base, _) = mock_provider(5, &[]).await?;
        let app = app(base);
        assert_eq!(
            request(&app, "disabled", "acme").await.unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "unknown", "acme").await.unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "good", "acme/labs").await.unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(&app, "", "acme").await.unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        Ok(())
    }

    #[tokio::test]
    async fn one_key_cannot_exhaust_the_global_limit() -> Result<()> {
        let (base, _) = mock_provider(5, &[]).await?;
        let app = app(base);
        for _ in 0..KEY_LIMIT_PER_MINUTE {
            request(&app, "good", "acme").await.unwrap();
        }
        assert_eq!(
            request(&app, "good", "acme").await.unwrap_err(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            request(&app, "disabled", "acme").await.unwrap_err(),
            StatusCode::FORBIDDEN
        );
        Ok(())
    }
}
