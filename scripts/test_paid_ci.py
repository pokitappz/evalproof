"""Exercise paid CLI outcomes with a separately compiled TEST public key.

Create a lease for organization "offline-test" with examples/license_fixture.rs
(cargo run --example license_fixture -- LEASE_PATH offline-test), then compile with that public key
and EVALPROOF_LICENSE_ENVIRONMENT=test. This never contacts a billing service.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--lease", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    environment = {
        **os.environ,
        "EVALPROOF_ENTITLEMENT": args.lease.read_text(),
        "EVALPROOF_ORG": "offline-test",
    }
    environment.pop("EVALPROOF_LICENSE_KEY", None)
    with tempfile.TemporaryDirectory(prefix="evalproof-paid-test-") as directory:
        root = Path(directory)
        def call(arguments, expected, env=environment):
            result = subprocess.run([str(binary), *arguments], cwd=root, env=env, capture_output=True, text=True)
            assert result.returncode == expected, (arguments, result.returncode, result.stderr, result.stdout)
        call(["init", "--demo"], 0)
        call(["check"], 1)
        suite = json.loads((root / "evalproof.json").read_text())
        suite["adapter"]["command"][2] = "grader.py:strict"
        (root / "evalproof.json").write_text(json.dumps(suite))
        call(["check"], 0)
        no_lease = dict(environment)
        no_lease.pop("EVALPROOF_ENTITLEMENT")
        call(["check"], 2, no_lease)
        call(["diagnose"], 2, {**no_lease, "CI": "true"})
        call(["diagnose"], 0, {**environment, "CI": "true"})
        call(["check"], 2, {**environment, "EVALPROOF_ORG": "another-org"})
        suite["adapter"].update(mode="llm", fresh_calls=True, max_cost_microusd=10000)
        suite["policy"]["budget_microusd"] = 80000
        (root / "evalproof.json").write_text(json.dumps(suite))
        call(["check", "--run-id", "paid-resume"], 2)
        call(["check", "--run-id", "paid-resume", "--require-resume", "--budget-microusd", "5000000"], 0)
        final = json.loads((root / ".evalproof/latest.json").read_text())
        assert len(final["observations"]) == 320 and final["reserved_microusd"] == 3200000
    print("Paid CLI verified: FAIL=1, PASS=0, unavailable license=2, unlicensed CI diagnose=2, wrong org=2, INCONCLUSIVE=2, resumed LLM PASS=0.")


if __name__ == "__main__":
    main()
