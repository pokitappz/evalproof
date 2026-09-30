# Development verification record

Local verification on September 15, 2026, updated September 30, 2026 for organization-bound licensing, CI gating of free diagnostics, the purchase path and the GitHub Action. This records development evidence, not commercial launch acceptance.

## Correctness

| Check | Result |
| --- | --- |
| `cargo test --all-targets` | 15 library unit tests, 4 entitlement-service tests against a mock Lemon Squeezy server, and 16 CLI integration tests passed |
| `cargo clippy --all-targets -- -D warnings` | Passed without warnings |
| Python adapter unit tests | 4 passed |
| Promptfoo 0.123.0 adapter conformance tests | 7 passed using the supported minimal installation |
| `python3 scripts/verify_statistics.py` | 90 independently computed interval references agreed within 1.9e-12; 18 exact error-bound scenarios passed |
| `scripts/test_paid_ci.py` with a separately compiled test public key | PASS=0, FAIL=1, missing license=2, unlicensed diagnose in CI=2, licensed diagnose in CI=0, lease for another organization=2, INCONCLUSIVE=2; resumed model-mode run passed with 320 observations and $3.20 reserved |
| `scripts/smoke_package.py` against a test-key release build | Passed, including with `CI` and `GITHUB_ACTIONS` set in the calling environment |
| Entitlement container | Built successfully; health returned `ok`; empty license request returned HTTP 400 |
| Final macOS x64 Python wheel and npm packages | Both installed and passed the complete demo-to-fixed-grader workflow; artifact checksums verified |

Integration coverage includes a weak grader accepting proven incorrect outputs, a strict grader passing, exceptions and timeouts remaining errors, unconfirmed contracts, durable budget resume, incomplete in-flight calls after a crash, cache dependency changes, cached-evidence tampering, single-case cache isolation, missing CI retry evidence, HTML escaping, and balanced sampling when critical metrics outnumber workers.

The statistical reference uses direct binomial sums and bisection independently of the Rust beta-distribution implementation. This numerical cross-check does not replace the independent human review required in [statistics.md](statistics.md). Model-mode tests used deterministic mock callbacks; no live model calls or paid provider charges were incurred.

## Performance

Command: `python3 scripts/benchmark.py --binary PATH_TO_RELEASE_BINARY`.

Workload: 100 frozen invoice fixtures, 1,000 incorrect mutations, 100 valid controls, a trivial Python grader, and eight persistent workers. Measurements came from this x86_64 macOS development machine; CPU and memory were not constrained to the proposed two-vCPU acceptance environment.

| Measurement | Before | After |
| --- | --- | --- |
| Complete deterministic run | 15.4-17.5 seconds | 7.9-8.9 seconds |
| Identical run with explicitly enabled safe caching | 1.46-1.50 seconds | 0.183 seconds |

Profiling identified repeated durable snapshot writes and unbuffered evidence reads as the main costs. The authoritative run is still atomically synchronized before dispatch and after each observation batch. The convenience `latest.json` snapshot is now written at run start and completion. Buffered reads reduced measured cached JSON read time from 1,310 ms to 17 ms. Optional `EVALPROOF_PROFILE=1` output contains timing counters only.

These are local observations, not latency guarantees for real graders. Remote grading latency, filesystem synchronization, case size, concurrency and the number of required critical metrics affect runtime.

## Safety and privacy review

| Area | Implemented protection and evidence |
| --- | --- |
| False confidence from grader failures | Adapter failures, timeouts and malformed protocol responses cannot count as caught mutations; regression tests exercise these paths |
| Cached result integrity | Regenerated cases, observation sequences and recalculated bounds validate saved evidence; subset diagnostics have distinct fingerprints |
| Budget and retry integrity | Cost reservations are durable before dispatch; incomplete in-flight calls require review; CI retries can require restored evidence |
| Input resource use | Fixture, rule, output, generated-case and protocol response limits bound allocations |
| Customer data in reports | Payloads and reasons are escaped; HTML has a restrictive CSP and no scripts or remote assets; CI summaries omit payloads and reasons |
| License authorization | Ed25519 signatures, audience, environment, expiry and organization are checked; upstream validation is bound to the configured store, product and variant; each organization is one Lemon Squeezy instance, capped by the activation limit; activation is serialized so concurrent jobs cannot consume two seats; rate limits apply per key and globally |
| Rust dependencies | `cargo audit --json`: 243 dependencies, no reported advisories or warnings at verification time |
| Supported Node dependencies | `npm audit --omit=optional --json`: no reported vulnerabilities at verification time |

Promptfoo's complete optional dependency tree reported seven high-severity advisories in integrations unused by the adapter. The supported installation excludes optional dependencies and install scripts; all seven conformance tests passed with that installation. Installing other upstream integrations changes the audited dependency set. See [integrations.md](integrations.md).

The report was checked through source inspection and generated-output tests. No browser screenshots or rendered visual review were performed.

## Releasing the candidate

The local host target is `x86_64-apple-darwin`. The package builder checks executable headers against the requested platform before producing npm archives or platform-specific Python wheels. Checksums accompany the artifacts. A package tagged for another architecture is rejected.

`scripts/smoke_package.py` runs the installed CLI through demo initialization, a weak-grader finding, report creation, preflight checks and a strict-grader passing result. The release-candidate workflow installs each generated wheel and runs this check before uploading artifacts. Those remote platform jobs have not been executed in this workspace.

The final local wheel was installed in an isolated Python environment. The npm wrapper and native archive were installed together without optional upstream integrations. Both passed the smoke check, and the installed npm executable matched the final release binary byte-for-byte. The installed Python demo exposed the weak grader in seven calls and 0.21 seconds; its persistent report is `.evalproof/installed-demo/.evalproof/report.html` in the local checkout.

The GitHub Action (`action.yml`) was checked as valid YAML; it has not been executed on GitHub, and `actionlint` was not available locally. The pricing site was not deployed.

Nothing has been published or deployed. Production signing configuration, merchant activation and license lifecycle testing, all-platform installation results, live-provider cost and failure checks, independent statistical review and self-service onboarding trials remain required. The complete checklist is in [release.md](release.md).
