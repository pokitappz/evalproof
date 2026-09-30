"""Persistent protocol-v1 adapter. The callback receives no mutation labels.

Usage: python3 adapter.py path/to/grader.py:function
Callback: grade(output, context) -> bool or {verdict/reason/score/cost_microusd}.
Numeric-only grades require an explicit threshold in a customer wrapper.
"""
import asyncio
import contextlib
import importlib.util
import inspect
import io
import json
import math
from pathlib import Path
import sys

MAX_LINE = 1024 * 1024


def normalize(value):
    if isinstance(value, bool):
        return {"verdict": "accept" if value else "reject"}
    if not isinstance(value, dict):
        raise ValueError("Return bool or an explicit verdict object; wrap numeric scores with a threshold")
    allowed = {"verdict", "pass", "reason", "score", "cost_microusd", "error"}
    if set(value) - allowed:
        raise ValueError("Unsupported grading result field")
    if value.get("error") is not None:
        return {"verdict": "error", "reason": "Grader reported an execution error"}
    verdict = value.get("verdict")
    if verdict is None and type(value.get("pass")) is bool:
        verdict = "accept" if value["pass"] else "reject"
    if verdict not in ("accept", "reject", "error"):
        raise ValueError("Missing explicit verdict")
    if "pass" in value and (type(value["pass"]) is not bool or (value["pass"] != (verdict == "accept"))):
        raise ValueError("Conflicting verdict and pass")
    result = {"verdict": verdict}
    if "reason" in value:
        if not isinstance(value["reason"], str) or len(value["reason"].encode()) > 16384:
            raise ValueError("Invalid reason")
        result["reason"] = value["reason"]
    if "score" in value:
        if type(value["score"]) not in (int, float) or not math.isfinite(value["score"]):
            raise ValueError("Invalid score")
        result["score"] = value["score"]
    if "cost_microusd" in value:
        if type(value["cost_microusd"]) is not int or not 0 <= value["cost_microusd"] <= 2**64-1:
            raise ValueError("Invalid cost")
        result["cost_microusd"] = value["cost_microusd"]
    return result


def load(target):
    path, sep, name = target.rpartition(":")
    if not sep or not name.isidentifier():
        raise ValueError("Expected grader.py:function")
    path = Path(path).resolve()
    sys.path.insert(0, str(path.parent))
    spec = importlib.util.spec_from_file_location("evalproof_customer_grader", path)
    if spec is None or spec.loader is None:
        raise ValueError("Cannot load grader")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    callback = getattr(module, name)
    if not callable(callback):
        raise ValueError("Grader is not callable")
    return callback


def main():
    protocol = sys.stdout
    # Redirect callback prints, including import-time prints, away from the protocol.
    # Do not log customer payloads to CI stderr by default.
    with open(__import__("os").devnull, "w") as sink:
        try:
            with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
                callback = load(sys.argv[1])
        except Exception:
            return 2
        protocol.write('{"version":1,"ready":true,"adapter":"python"}\n')
        protocol.flush()
        # One persistent event loop supports async SDK clients across invocations.
        with asyncio.Runner() as runner:
            while True:
                raw = sys.stdin.buffer.readline(MAX_LINE + 1)
                if not raw:
                    return 0
                if len(raw) > MAX_LINE:
                    return 2
                request = None
                try:
                    request = json.loads(raw)
                    if request.get("version") != 1 or type(request.get("id")) is not int:
                        return 2
                    with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
                        value = callback(request["output"], request["context"])
                        if inspect.isawaitable(value):
                            value = runner.run(value)
                        result = normalize(value)
                except Exception:
                    result = {"verdict": "error", "reason": "Python grader failed or returned an invalid result"}
                if not isinstance(request, dict) or "id" not in request:
                    return 2
                protocol.write(json.dumps({"version": 1, "id": request["id"], "result": result}, allow_nan=False) + "\n")
                protocol.flush()


if __name__ == "__main__":
    raise SystemExit(main())
