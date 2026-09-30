# EvalProof for Promptfoo

Run `evalproof init --promptfoo promptfooconfig.yaml --fixtures outputs.json`, review the generated contracts, then run `evalproof diagnose`.

Requires Node 22.22 or newer and Promptfoo 0.123.0. Node 24 is recommended. Platform executables are distributed as optional npm dependencies. This source checkout must be packaged before installation as a binary distribution.

No Rust compiler or Python runtime is needed unless your own grader needs Python. Unsupported configurations return errors rather than silently omitting checks. See the repository integration notes for the tested assertion subset.
