# EvalProof

Find the incorrect outputs your AI graders accept. EvalProof changes frozen JSON outputs, proves each change violates a reviewed contract, and runs your existing grader against it. Everything executes on your machine or CI infrastructure.

**Status: working development candidate, not a publicly launched service.** The engine, Python and Promptfoo adapters, HTML reports, statistical gates, entitlement service and packaging workflows are implemented. Production billing, all-platform release validation, independent statistical review and customer onboarding trials are release prerequisites. Nothing has been deployed or published.

## Try the complete demo

From this repository:

```sh
cargo build --bin evalproof
cargo run --bin evalproof -- init --demo --output examples/demo/evalproof.json
cargo run --bin evalproof -- diagnose --suite examples/demo/evalproof.json
```

The report is `examples/demo/.evalproof/report.html`. It shows that checking only for `invoice_id` misses incorrect invoice totals and identifiers. Change `grader.py:weak` to `grader.py:strict` in the generated configuration and rerun. The strict grader passes.

`diagnose` is free on developer machines and returns zero for a completed diagnostic even when it finds grader defects. Inside CI (`CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `JENKINS_URL` or `TF_BUILD` set) it requires a Team entitlement, like `check`. `check` is the paid CI interface: `0` means PASS, `1` means FAIL, and `2` means INCONCLUSIVE or ERROR. A local source build without the release licensing configuration can run diagnostics but cannot authorize paid gates or CI runs.

## Connect your grader

Save existing frozen outputs as an array:

```json
[{"id":"invoice-1","output":{"invoice_id":"INV-42","total":125},"context":{}}]
```

For Python:

```sh
evalproof init --python grader.py:grade --fixtures outputs.json
```

The function receives `(output, context)` and returns a boolean or an explicit object:

```python
def grade(output, context):
    return {"verdict": "accept" if output["total"] == 125 else "reject",
            "reason": "Checks the expected total"}
```

Exceptions are execution errors. Numeric-only results require an explicit threshold in your wrapper. Async functions are supported with a persistent event loop. Python 3.11 or later is required; your Python project needs no Node installation.

For Promptfoo, add `"test_index":0` to each fixture's context, install Promptfoo 0.123.0 in your project, then:

```sh
evalproof init --promptfoo promptfooconfig.yaml --fixtures outputs.json
```

Optional context fields select `prompt_index`, `provider_index`, and `output_format` (`text`, the default, or `json`). Review supported configurations in [integration notes](docs/integrations.md). No application generation calls are made: a frozen provider supplies the captured output, while the native evaluator runs the assertions and transforms.

Review the inferred requirements, set the grader mode, and set `confirmed` to `true`. Inferred exact values are proposals, not automatically accepted truth. Then:

```sh
evalproof doctor
evalproof diagnose
```

## Contracts and daily checks

The versioned JSON configuration lives in Git. Each fixture has independently reviewed rules using JSON pointers. See [configuration and protocol](docs/configuration.md).

Deterministic gates require every incorrect case to be rejected and every valid case accepted. LLM gates use bounded independent sampling with multiple-comparison and stopping corrections. They never count a crashed grader as a caught mistake. See [statistical policy](docs/statistics.md).

For an LLM suite, set `adapter.mode` to `llm`, `fresh_calls` to `true`, provide a stable `identity`, and declare `max_cost_microusd` for the entire callback. One dollar is 1,000,000 microdollars. This reservation must include nested calls and retries. An opaque callback cannot be assigned a trustworthy dollar cap automatically.

```sh
evalproof diagnose --run-id candidate-42
evalproof diagnose --run-id candidate-42 --budget-microusd 10000000 --time-limit-seconds 600
```

The second command resumes the same observations and sampling plan. Increasing a budget never resets unfavorable evidence. Calls stop at the predeclared maximum sample count; exhausted evidence can remain inconclusive. A run interrupted with calls in flight requires review and a new run because the tool cannot recover the missing remote responses.

Free diagnostics support complete local reports. EvalProof Team adds CI use, the blocking `check` interface, the GitHub Action with pull request comments and automatic default-branch baseline comparison. It is priced for launch at **$79 per organization per month** with a 14-day free trial, unlimited projects, repositories and runs on customer compute. `evalproof license buy` prints (and on an interactive terminal opens) the checkout link; `evalproof license portal` opens billing management. Model calls are billed by your own provider. Commercial willingness to pay remains to be validated.

The GitHub Action at the repository root (`action.yml`) installs a pinned release, restores the latest successful default-branch baseline artifact, runs `check`, posts or updates one pull request comment containing only the payload-free summary, and optionally saves new default-branch evidence. Copy and adapt [the customer GitHub workflow](docs/customer-workflow.yml). Run deterministic suites on every PR and LLM suites on relevant changes plus a nightly schedule. Start conservatively by running all configured suites. Cross-run caching is disabled until you explicitly declare `cache_safe` and all dependency/environment inputs. Matching evidence expires after 24 hours. Single-case diagnostics have separate cache identities.

Use `check --baseline path/to/previous-run.json` to compare against saved default-branch evidence. Comparisons never waive old failures. Stochastic problems absent from a later sample are labeled "not observed", not "fixed".

## Privacy and licensing

Fixtures, grader outputs and reports never upload to EvalProof. Your configured LLM providers receive the grading requests. HTML reports and JSON evidence contain your data; keep them in approved storage. Reports have no remote assets, scripts or telemetry. CI summaries omit payloads and grader reasons.

The only hosted component is the entitlement service. Lemon Squeezy handles checkout and subscription licenses. Each subscription covers one organization: the service validates the purchased product, records the organization as a Lemon Squeezy license key instance (reusing it on later refreshes), and signs a seven-day offline lease bound to that organization. The organization comes from `EVALPROOF_ORG`, `GITHUB_REPOSITORY_OWNER` or GitLab's `CI_PROJECT_ROOT_NAMESPACE`. CI can use `EVALPROOF_ENTITLEMENT`, or refresh a lease using `EVALPROOF_LICENSE_KEY`; a lease for another organization is rejected. Release binaries pin the service URL and public key at compile time. See [release and operations](docs/release.md).

## Verify and package

See the [verification record](docs/verification.md) for measured results and remaining launch checks.

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
python3 -m unittest discover -s packages/python/tests -v
python3 scripts/verify_statistics.py
npm ci --ignore-scripts --prefix packages/node
npm test --prefix packages/node
```

`scripts/package.py` creates platform npm packages and Python wheels from an existing native binary; it never publishes them. The release workflow builds macOS x64/ARM64, Linux x64/ARM64 and Windows x64 candidates. Public package names are working names until ownership is confirmed.
