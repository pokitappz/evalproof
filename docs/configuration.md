# Configuration and protocol v1

`evalproof.json` is strict JSON. Unknown fields are rejected. `init` proposes exact-value and required-field rules and leaves real imports unconfirmed. Set `confirmed` only after review. Changing contracts or policies requires the same review as changing tests.

```json
{
  "version": 1,
  "name": "Invoice grader",
  "confirmed": true,
  "adapter": {
    "command": ["python3", "evalproof-python.py", "grader.py:grade"],
    "mode": "deterministic",
    "dependencies": ["evalproof-python.py", "grader.py", "requirements.txt"],
    "environment": [],
    "cache_safe": false,
    "identity": "invoice-grader-v1"
  },
  "fixtures": [{
    "id": "invoice-1",
    "output": {"invoice_id": "INV-42", "total": 125},
    "context": {},
    "semantic_json": false,
    "rules": [
      {"id": "total-present", "path": "/total", "kind": "required"},
      {"id": "total-value", "path": "/total", "kind": "number", "value": "125.00", "tolerance": "0.01", "critical": true}
    ]
  }]
}
```

## Contract rules

Every rule has `id`, an RFC 6901 `path`, an optional `critical` flag, and `kind`:

| Kind | Fields | Meaning |
|---|---|---|
| `required` | none | Pointer must exist. Present null is distinct from missing. |
| `type` | `value` | `null`, `boolean`, `number`, `string`, `array`, or `object`. |
| `exact` | `value` | Exact JSON value, with decimal numeric equivalence. |
| `enum` | `values` | Allowed JSON values. |
| `number` | decimal strings `value`, `tolerance` | Absolute decimal tolerance, inclusive boundaries. |
| `cardinality` | `min`, `max` | Inclusive array size limits. |
| `unique` | optional `key` pointer | Array values, or identified fields, must be unique. |
| `members` | `key` pointer, `values` | Exact multiset of item identities, independent of order. |
| `unordered` | optional `key` pointer | Declares order unimportant and permits valid reordered controls. |

Constraints other than `required` apply only when the pointer exists. Add `required` when absence is incorrect. No natural-language contract inference or fuzzy equivalence is silently accepted. Decimals exceeding the engine's supported precision cause an error instead of rounding.

`unordered` does not cancel an incompatible `exact` rule. All valid controls must satisfy every rule. Use `members` with `unordered` and any required per-item requirements when order should not matter.

The mutation operator pack is part of the engine version. A rule without a provable applicable incorrect mutation is listed as unsupported and prevents a passing gate. Baseline fixtures violating their own contracts stop before grading.

## Execution policy

Defaults: tolerance `0.05`, confidence `0.99`, `budget_microusd: 5000000`, `time_limit_seconds: 300`, `call_timeout_seconds: 30`, `concurrency: 8`, and `max_cases: 10000`.

LLM mode additionally requires `fresh_calls: true`, a nonempty adapter `identity`, and `max_cost_microusd`. This is an owner-declared upper bound on the complete callback, including all model calls and internal retries. Actual charges outside that declared bound cannot be prevented by this tool. Missing actual usage is displayed as unreported, never as free usage. Promptfoo's generation cost is not misrepresented as grading cost.

The runner reserves cost before starting a batch and stops dispatching when insufficient budget remains. It also requires enough remaining time for the callback timeout before dispatching. Provider errors are terminal errors; valid budget exhaustion can be resumed within the same predeclared sample plan.

Run IDs contain only letters, digits, hyphens and underscores. Reusing an ID with different grader inputs fails. `--require-resume` makes a CI retry fail if its prior evidence is missing. The local state directory is locked against concurrent writers. The authoritative run file is durably and atomically replaced before dispatch and after each completed batch. `latest.json` is published at run start and completion, avoiding duplicate disk-sync work; it is not a live progress feed.

For caching, list all grader/helper code, dependency locks, adapter files, model identifiers and relevant environment-variable names. Directory fingerprints exclude generated directories such as `.git`, `node_modules`, `target`, `.venv` and `.evalproof`. Fingerprint installed dependency versions through lockfiles. List an otherwise excluded file explicitly if it affects grading. Only set `cache_safe` when those boundaries and grader purity are known. The engine executable is also hashed. Artifacts are trusted local test evidence, not signed attestations against a malicious repository author.

## Adapter protocol

The engine starts `adapter.command` directly without a shell. Working directory is the suite's directory. Workers are persistent; each handles one request at a time. They inherit the environment so existing provider credentials continue to work. Treat graders and configuration as executable trusted repository code.

The adapter sends one handshake line:

```json
{"version":1,"ready":true,"adapter":"python"}
```

Each grading request is one JSON line:

```json
{"version":1,"id":12,"output":{"total":1250},"output_text":"{\"total\":1250}","context":{}}
```

Each response is one JSON line:

```json
{"version":1,"id":12,"result":{"verdict":"reject","reason":"Incorrect total","score":0,"cost_microusd":1000}}
```

`verdict` is `accept`, `reject`, or `error`; `reason`, `score` and `cost_microusd` are optional. Scores must be finite and costs nonnegative integers. Mutation labels, contract proofs and expected verdicts never enter the grading request. Requests are limited to 1 MiB; responses to 2 MiB; reasons to 16 KiB. Mismatched IDs, malformed output, nonfinite scores, process exits and timeouts are errors.

Reports preserve all observations. A stochastic mismatch can remain visible in a passing suite because the approved policy tolerates a bounded error rate. This is different from a deterministic gate, where any mismatch fails.
