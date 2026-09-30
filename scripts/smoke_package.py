"""Verify an installed CLI's first-run workflow without provider or billing calls."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

CI_VARIABLES = {"CI", "GITHUB_ACTIONS", "GITLAB_CI", "BUILDKITE", "CIRCLECI", "JENKINS_URL", "TF_BUILD"}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", nargs="+", default=["evalproof"])
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="evalproof-install-") as directory:
        root = Path(directory)

        # Diagnose is free on developer machines only, so simulate one even when
        # this smoke test itself runs inside a CI job.
        environment = {
            name: value
            for name, value in os.environ.items()
            if name not in CI_VARIABLES
        }

        def call(*arguments):
            result = subprocess.run(
                [*args.cli, *arguments], cwd=root, env=environment, capture_output=True, text=True
            )
            if result.returncode:
                raise RuntimeError(result.stdout + result.stderr)

        call("--version")
        call("init", "--demo")
        call("diagnose")
        evidence = root / ".evalproof/latest.json"
        assert json.loads(evidence.read_text())["status"] == "FAIL"
        assert (root / ".evalproof/report.html").is_file()
        suite = root / "evalproof.json"
        config = json.loads(suite.read_text())
        config["adapter"]["command"][-1] = "grader.py:strict"
        suite.write_text(json.dumps(config))
        call("doctor")
        call("diagnose")
        assert json.loads(evidence.read_text())["status"] == "PASS"
    print("Installed CLI verified: setup, weak-grader finding, HTML report, doctor, strict-grader PASS.")


if __name__ == "__main__":
    main()
