#!/usr/bin/env python3
"""Verify actual Cargo compiler-artifact features, not only a planned tree."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


FORBIDDEN = frozenset(("fault-injection", "test-support"))


def verify(lines) -> dict:
    selected = {}
    finished = False
    for line in lines:
        message = json.loads(line)
        reason = message["reason"]
        if reason == "compiler-message":
            diagnostic = message["message"]
            if diagnostic["level"] in {"error", "warning"}:
                raise ValueError(f"compiler {diagnostic['level']}: {diagnostic['message']}")
        elif reason == "build-finished":
            if not message["success"]:
                raise ValueError("Cargo reported an unsuccessful build")
            finished = True
        elif reason == "compiler-artifact":
            target = message["target"]
            name = target["name"]
            if name not in {"storage_engine", "history-scale-driver"}:
                continue
            features = message["features"]
            if FORBIDDEN.intersection(features):
                raise ValueError(f"forbidden actual features on {name}: {features}")
            profile = message["profile"]
            if profile["test"] or profile["opt_level"] != "3" or profile["debug_assertions"]:
                raise ValueError(f"expected ordinary release artifact for {name}: {profile}")
            expected_kind = "lib" if name == "storage_engine" else "bin"
            if target["kind"] != [expected_kind]:
                raise ValueError(f"unexpected target kind for {name}: {target['kind']}")
            selected[name] = {
                key: message[key]
                for key in ("package_id", "manifest_path", "features", "profile", "target", "fresh")
            }
    if not finished or set(selected) != {"storage_engine", "history-scale-driver"}:
        raise ValueError("missing successful completion or actual engine/driver artifacts")
    return {"verified": True, "artifacts": selected}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    arguments = parser.parse_args()
    with arguments.input.open() as source:
        print(json.dumps(verify(source), indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
