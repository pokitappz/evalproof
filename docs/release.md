# Release and operations

## Development candidate versus production release

The source build is a functioning local diagnostic. Production paid CI requires a configured merchant product, an HTTPS entitlement service, and binaries built with its public key. No development bypass is accepted by the production licensing verifier.

Before publication, confirm ownership of the working `evalproof` / `@evalproof` package names and choose the public service hostname. Do not publish an unreviewed statistical gate. Complete the independent review in `statistics.md`, the five-platform installation matrix and live-provider operational tests first.

## Billing service

Create a $79/month team subscription product in Lemon Squeezy with:

- License keys enabled, **activation limit 1** (one organization per subscription). Raise the limit per customer, or sell a quantity-based variant, for multi-organization buyers.
- A 14-day free trial. Trial subscriptions are active, so their license keys validate (see [License keys and subscriptions](https://docs.lemonsqueezy.com/help/licensing/license-keys-subscriptions)).
- Separate test and live products.

Store activation is an external prerequisite for live sales and requires the privacy and terms pages. Checkout and the customer billing portal are hosted by Lemon Squeezy.

### How organization binding works

The service stays stateless; Lemon Squeezy is the record. For each `POST /v1/entitlements` with `{"license_key", "org"}` it:

1. Calls [`/v1/licenses/validate`](https://docs.lemonsqueezy.com/api/license-api/validate-license-key) and checks store, product and variant.
2. Lists the key's instances with [`GET /v1/license-key-instances?filter[license_key_id]=`](https://docs.lemonsqueezy.com/api/license-key-instances/list-all-license-key-instances) using `LEMONSQUEEZY_API_KEY`.
3. Reuses an instance named after the organization, or calls [`/v1/licenses/activate`](https://docs.lemonsqueezy.com/api/license-api/activate-license-key) with `instance_name=<org>`. Activation responses return HTTP 200 with `activated: false` at the limit; the service returns 403 with a billing-portal hint.
4. Signs a version 2 lease containing the organization.

Repeated CI refreshes therefore never consume activations. Customers move a subscription to another organization by deactivating the old instance from their Lemon Squeezy order page. The number of instances per key is also the activation metric for the demand test, with no telemetry in the client.

### Deploy the service on Fly.io

The included `fly.toml` runs one always-on machine: the upstream rate limiter (50 requests per minute overall, 6 per key) and the activation lock are process-local, so do not scale out.

```sh
fly launch --no-deploy --copy-config          # once; keeps fly.toml
fly secrets set \
  EVALPROOF_SIGNING_KEY_HEX=<32 random bytes encoded as 64 hex characters> \
  EVALPROOF_LICENSE_ENVIRONMENT=test \
  LEMONSQUEEZY_API_KEY=<API key; only needs to list license key instances> \
  LEMONSQUEEZY_STORE_ID=<numeric ID> \
  LEMONSQUEEZY_PRODUCT_ID=<numeric ID> \
  LEMONSQUEEZY_VARIANT_ID=<numeric ID>
fly deploy
fly certs add licensing.YOUR_DOMAIN           # then add the DNS records it prints
```

Use a separate Fly app for the test and live environments. Keep the signing key only in the Fly secret store. The service does not log requests or retain customer records; Fly's proxy logs do not include request bodies. `/health` is the health endpoint.

### Deploy the pricing site on Cloudflare Pages

`site/` is static HTML and CSS with no scripts, trackers or remote assets; `site/_headers` sets a strict CSP. Before deploying, replace `CHECKOUT_URL` (the Lemon Squeezy checkout link for the Team variant), `PORTAL_URL` (`https://YOUR_STORE.lemonsqueezy.com/billing`) and `SUPPORT_EMAIL`, and replace the draft privacy and terms text with reviewed legal text.

```sh
npx wrangler pages deploy site --project-name evalproof
```

The CLI's fallback purchase link is `PRICING_URL` in `src/license.rs` (currently the working domain `evalproof.dev`); update it once the domain is confirmed.

Build release clients with these compile-time settings:

```text
EVALPROOF_LICENSE_PUBLIC_KEY=<corresponding Ed25519 public key in hex>
EVALPROOF_LICENSE_ENVIRONMENT=live
EVALPROOF_ENTITLEMENT_URL=https://YOUR_SERVICE/v1/entitlements
EVALPROOF_CHECKOUT_URL=<Lemon Squeezy checkout link for the Team variant>
EVALPROOF_PORTAL_URL=https://YOUR_STORE.lemonsqueezy.com/billing
```

The public key, endpoint and billing links can be public repository variables. The private signing key must never enter customer packages or CI jobs running customer graders. Set the same public build variables on the release-candidate workflow.

Test the full test-mode purchase, trial, receipt, first-organization activation, repeated refresh, second-organization refusal, instance deactivation, expiry, invalid product, disabled license and service outage flows before enabling live checkout. The public License API response is checked without retaining names or emails. Offline leases expire after seven days, so revocation can take up to that long to affect an offline customer.

If the signing key is compromised, deploy a new signing key and publish clients with the new public key. Existing clients require an update; this initial release has no remote key rotation mechanism.

## GitHub Action

`action.yml` at the repository root is a composite action. Publishing it means tagging a release of this repository (for example `v0.1.0`) and optionally listing it on the GitHub Marketplace. It installs the pinned PyPI package, so publish the Python wheels first. `smoke_package.py` clears CI variables because unlicensed `diagnose` is refused inside CI.

## Packaging

The manual release workflow assembles artifacts but does not publish or deploy. Linux binaries are compiled for musl, avoiding dynamic glibc dependencies; Python wheels target compatible manylinux hosts. macOS minimum deployment version is 11.0. Windows uses the native MSVC target. The packages contain executables, so Python wheels use `py3-none-<platform>` rather than a false universal tag.

To package a locally built binary:

```sh
python3 -m pip install build
python3 scripts/package.py --target aarch64-apple-darwin --binary PATH_TO_RELEASE_BINARY
```

Inspect `dist/<target>/SHA256SUMS` and install each package in a fresh environment before publishing. Publish platform npm packages before the shared wrapper so optional dependencies resolve. Pin released versions; do not overwrite an existing release. macOS signing/notarization and distribution-account ownership must be resolved before broad distribution if platform policy requires them.

## CI evidence and secrets

The customer workflow has read-only repository permissions and does not run credentialed graders from fork PRs. It never uses `pull_request_target`. Teams must approve the storage of fixture-bearing evidence before setting `EVALPROOF_CACHE_EVIDENCE=true` or `EVALPROOF_UPLOAD_REPORTS=true`. GitHub caches may be readable by repository contributors and forks under GitHub's cache access rules; do not enable them for sensitive public-repository fixtures.

Cached paths explicitly exclude entitlements and license keys. Retry caches use distinct attempt keys so newer observations are not replaced by an immutable first-attempt snapshot. `--require-resume` stops retries when evidence was not restored. For sensitive environments, use a private persistent runner or restore evidence through approved private storage.

The first workflow runs all suites conservatively. Optimize change detection only after declaring complete dependency boundaries. Nightly runs collect fresh evidence for provider drift. Baseline artifacts should come from the trusted default branch; a comparison never silently suppresses findings.

## Launch acceptance still requiring external evidence

- Independent review of the statistical implementation and its operational assumptions.
- Installation and execution on all five OS/architecture targets.
- Real-provider timeout, cost-bound, caching and model-version checks.
- Test-mode then live merchant/license lifecycle checks.
- Eight of ten target developers reaching their first useful diagnostic within five minutes without live assistance.
- Thirty-day demand test: ten activated teams, five weekly users, three paying customers.

These are acceptance activities, not claims established by local automated tests. Track opt-in activation and retention without collecting fixture payloads. The local product currently sends no analytics.
