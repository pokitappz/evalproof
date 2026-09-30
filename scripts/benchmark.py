"""Published benchmark: 100 small fixtures, 1000 mutations and 100 valid controls."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="evalproof-bench-") as directory:
        root = Path(directory)
        subprocess.run([str(binary), "init", "--demo"], cwd=root, check=True, capture_output=True)
        suite = json.loads((root / "evalproof.json").read_text())
        fixtures = []
        for i in range(100):
            output = {f"field{j}": i * 10 + j for j in range(5)}
            fixtures.append({"id": f"row-{i}", "output": output, "context": {"expected": output},
                             "rules": [{"id": k, "path": f"/{k}", "kind": "exact", "value": v} for k, v in output.items()]})
        suite["fixtures"] = fixtures
        suite["adapter"]["cache_safe"] = True
        (root / "evalproof.json").write_text(json.dumps(suite))
        (root / "grader.py").write_text("def weak(output, context):\n    return output == context['expected']\n")
        timings = []
        profiles = []
        for _ in range(2):
            start = time.perf_counter()
            result = subprocess.run([str(binary), "diagnose"], cwd=root, capture_output=True, text=True, check=True, env={**os.environ, "EVALPROOF_PROFILE":"1"})
            timings.append(time.perf_counter() - start)
            profiles.append([json.loads(line[8:]) for line in result.stderr.splitlines() if line.startswith("PROFILE ")])
        run = json.loads((root / ".evalproof/latest.json").read_text())
        assert run["status"] == "PASS" and len(run["cases"]) == 1100 and run["reused"]
        print(json.dumps({"fixtures": 100, "mutations": 1000, "controls": 100,
                          "first_run_seconds": timings[0], "cached_run_seconds": timings[1], "profiles": profiles}, indent=2))


if __name__ == "__main__":
    main()
