"""Assemble platform npm packages and ABI-independent Python wheels. No publishing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import struct
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "aarch64-apple-darwin": ("darwin", "arm64", "macosx_11_0_arm64"),
    "x86_64-apple-darwin": ("darwin", "x64", "macosx_11_0_x86_64"),
    "x86_64-unknown-linux-musl": ("linux", "x64", "manylinux_2_17_x86_64"),
    "aarch64-unknown-linux-musl": ("linux", "arm64", "manylinux_2_17_aarch64"),
    "x86_64-pc-windows-msvc": ("win32", "x64", "win_amd64"),
}


def executable_platform(binary):
    with binary.open("rb") as file:
        head = file.read(64)
        if head[:4] == b"\xcf\xfa\xed\xfe":
            cpu = struct.unpack_from("<I", head, 4)[0]
            return "darwin", {0x1000007: "x64", 0x100000C: "arm64"}.get(cpu)
        if head[:4] == b"\x7fELF" and head[4:6] == bytes((2, 1)):
            cpu = struct.unpack_from("<H", head, 18)[0]
            return "linux", {62: "x64", 183: "arm64"}.get(cpu)
        if head[:2] == b"MZ":
            file.seek(struct.unpack_from("<I", head, 60)[0])
            pe = file.read(6)
            if pe[:4] == b"PE\0\0":
                return "win32", {0x8664: "x64"}.get(struct.unpack_from("<H", pe, 4)[0])
    raise ValueError("Unrecognized executable format")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--skip-wheel", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error("binary must already be built for the specified target")
    platform, cpu, wheel_platform = TARGETS[args.target]
    if executable_platform(binary) != (platform, cpu):
        parser.error(f"binary architecture {executable_platform(binary)} does not match target {args.target}")
    suffix = ".exe" if platform == "win32" else ""
    destination = ROOT / "dist" / args.target
    destination.mkdir(parents=True, exist_ok=True)
    version = json.loads((ROOT / "packages/node/package.json").read_text())["version"]
    with tempfile.TemporaryDirectory(prefix="evalproof-package-") as temp:
        staging = Path(temp)
        native = staging / "native"
        (native / "bin").mkdir(parents=True)
        shutil.copy2(binary, native / "bin" / ("evalproof" + suffix))
        (native / "bin" / ("evalproof" + suffix)).chmod(0o755)
        # The Elastic License requires every copy to carry its terms.
        shutil.copy2(ROOT / "LICENSE", native / "LICENSE")
        (native / "package.json").write_text(json.dumps({
            "name": f"@evalproof/cli-{platform}-{cpu}", "version": version,
            "description": "EvalProof platform executable", "license": "Elastic-2.0",
            "os": [platform], "cpu": [cpu], "files": ["bin"],
        }, indent=2))
        npm = "npm.cmd" if os.name == "nt" else "npm"
        subprocess.run([npm, "pack", "--pack-destination", str(destination)], cwd=native, check=True)
        wrapper = staging / "node"
        shutil.copytree(ROOT / "packages/node", wrapper,
                        ignore=shutil.ignore_patterns("node_modules", "package-lock.json", "tests", "bin"))
        shutil.copy2(ROOT / "LICENSE", wrapper / "LICENSE")
        manifest = json.loads((wrapper / "package.json").read_text())
        manifest.pop("devDependencies", None)
        manifest.pop("scripts", None)
        manifest["optionalDependencies"] = {f"@evalproof/cli-{p}-{c}": version for p, c, _ in TARGETS.values()}
        (wrapper / "package.json").write_text(json.dumps(manifest, indent=2))
        subprocess.run([npm, "pack", "--pack-destination", str(destination)], cwd=wrapper, check=True)
        if not args.skip_wheel:
            package = staging / "python"
            shutil.copytree(ROOT / "packages/python", package,
                            ignore=shutil.ignore_patterns("__pycache__", "*.egg-info", "dist", "build", "bin"))
            shutil.copy2(ROOT / "LICENSE", package / "LICENSE")
            (package / "evalproof/bin").mkdir(parents=True)
            shutil.copy2(binary, package / "evalproof/bin" / ("evalproof" + suffix))
            (package / "evalproof/bin" / ("evalproof" + suffix)).chmod(0o755)
            subprocess.run([os.environ.get("PYTHON", sys.executable), "-m", "build", "--wheel", "--outdir", str(destination)],
                           cwd=package, check=True, env={**os.environ, "EVALPROOF_WHEEL_PLATFORM": wheel_platform})
    hashes = []
    for file in sorted(destination.iterdir()):
        if file.suffix in (".tgz", ".whl"):
            hashes.append(f"{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.name}")
    (destination / "SHA256SUMS").write_text("\n".join(hashes) + "\n")
    print(f"Reviewable release artifacts: {destination}")


if __name__ == "__main__":
    main()
