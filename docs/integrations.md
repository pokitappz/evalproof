# Verified integration behavior

Checked against installed packages and primary documentation on 2026-09-15.

## Promptfoo 0.123.0

The adapter uses the public `evaluate()` API. It supplies frozen outputs and a fallback frozen provider, then reads `toEvaluateSummary()`. Assertions, test/default aggregation, thresholds and output transforms remain native Promptfoo behavior. `cache: false`, `sharing: false` and `writeLatestResults: false` are set explicitly.

The installed package differs from examples in its documentation:

- The actual `assertions.runAssertions()` reads assertions from `test`, rather than the example's separate `assertions` parameter. EvalProof avoids reimplementing this path.
- The internal `providerOutput` branch tests truthiness. A frozen fallback provider prevents falsey supplied outputs from triggering generation.
- A failed assertion can populate a row's `error` field. That field alone does not mean execution failed. EvalProof checks `failureReason`, nested `metadata.graderError`, and verified JavaScript execution-error sentinels.
- Some Python assertion exceptions become plain rejections with no structured error flag. The Promptfoo adapter therefore rejects that configuration; use the native Python adapter for those graders.
- Result generation cost does not reliably include all model grading calls. The adapter does not report it as actual grading cost.

Supported v1 assertion families: equals, contains, icontains, contains-any, contains-all, starts-with, regex, is-json, JavaScript, llm-rubric and assertion sets, including supported inverse forms. Other families are rejected. LLM rubric assertions require LLM mode. Inline atomic tests are required; scenario expansion, trace assertions, extensions and external test datasets must be materialized or wrapped separately. Unsupported configuration produces an error rather than silently reducing coverage.

The adapter accepts JSON or YAML through Promptfoo's installed `js-yaml` dependency. File references resolve relative to the configuration. Promptfoo users must pin the package version and include lockfiles plus any referenced scripts in the suite's dependency fingerprint. The engine supports arrays and scalars, but the initial import wizard expects frozen structured JSON rather than unquoted free text.

Conformance tests exercise acceptance/rejection, transforms, numeric thresholds, default assertions, exceptions, inverse error tagging and falsey outputs. The adapter rejects untested Promptfoo versions until the conformance suite passes for that version.

The tested installation uses `npm ci --omit=optional --ignore-scripts`. Optional model, browser and archive integrations are unnecessary for frozen-output grading. The full upstream optional dependency tree currently includes npm advisories in archive and image-processing packages; those are absent from the tested minimal installation. All conformance tests pass without optional integrations. The CLI's Promptfoo peer dependency is optional to avoid installing that entire toolchain implicitly. For a minimal npm CLI installation, explicitly install the matching `@evalproof/cli-<platform>-<arch>` binary package alongside the wrapper when omitting optional dependencies.

Sources: [Node API](https://www.promptfoo.dev/docs/usage/node-api-reference/), [installation](https://www.promptfoo.dev/docs/installation/), [release notes](https://www.promptfoo.dev/docs/releases/). The installed package is the authority for the discrepancies above.

## Lemon Squeezy

The entitlement service uses the separate License API, not the JSON:API store-management endpoint. It calls `POST https://api.lemonsqueezy.com/v1/licenses/validate` with form-encoded `license_key` and `Accept: application/json`. It does not distribute a store-management API key.

The service requires `valid: true`, an active or inactive license, and matching store, product and variant IDs. Test and live deployments use distinct allowlisted product IDs, signing keys and environment claims. License activation or machine fingerprinting is unnecessary for ephemeral CI workers. The upstream limit is 60 requests/minute; a single service instance limits incoming validations to 50/minute.

Subscription license validity follows the subscription lifecycle. A previously issued offline entitlement can remain usable for up to seven days after cancellation, disabling or refund. This is the deliberate offline grace window. No subscription webhooks are needed for this initial service. All upstream errors produce generic responses without logging license keys or customer metadata.

Sources: [License API](https://docs.lemonsqueezy.com/api/license-api), [validation wire format](https://docs.lemonsqueezy.com/api/license-api/validate-license-key), [subscription licenses](https://docs.lemonsqueezy.com/help/licensing/license-keys-subscriptions), [test mode](https://docs.lemonsqueezy.com/help/getting-started/test-mode).

No credentials or live merchant configuration were available for verification. Live checkout and license lifecycle tests remain external release prerequisites. No dated deprecation for these license endpoints was identified during the documentation check.
