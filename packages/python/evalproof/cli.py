"""Launch the platform binary bundled in the installed wheel."""
import os
from pathlib import Path
import subprocess
import sys


def main():
    name = "evalproof.exe" if os.name == "nt" else "evalproof"
    binary = Path(__file__).parent / "bin" / name
    if not binary.is_file():
        print("This source checkout has no bundled binary. Build it using scripts/package.py.", file=sys.stderr)
        return 2
    environment = dict(os.environ)
    environment.setdefault("EVALPROOF_PYTHON", sys.executable)
    return subprocess.call([str(binary), *sys.argv[1:]], env=environment)
