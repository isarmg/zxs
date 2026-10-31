#!/usr/bin/env python3
"""Check early workflow env contexts using only the Python standard library.

This deliberately narrow guard is not a complete YAML or Actions validator.
It accepts the repository's block-mapping style and checks workflow/job env
before any runner (or Rust toolchain) is needed. GitHub remains the full parser.
Context rules: https://docs.github.com/en/actions/reference/workflows-and-actions/contexts#context-availability
"""
import argparse
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
KEY = re.compile(r"(?P<indent> *)(?P<key>[A-Za-z_][A-Za-z0-9_-]*):(?:\s+(?P<value>.*))?$")
EXPRESSION = re.compile(r"\$\{\{(.*?)\}\}", re.DOTALL)
TOKEN = re.compile(r"'(?:[^']|'')*'|[A-Za-z_][A-Za-z0-9_-]*|[.\[]")
CONTEXTS = {"github", "env", "vars", "job", "jobs", "steps", "runner",
            "secrets", "strategy", "matrix", "needs", "inputs"}
WORKFLOW_ENV = {"github", "secrets", "inputs", "vars"}
JOB_ENV = WORKFLOW_ENV | {"needs", "strategy", "matrix"}


def expression_contexts(expression):
    """Find context roots, ignoring string literals and nested property names."""
    previous = None
    for token in TOKEN.findall(expression):
        lowered = token.lower()
        if lowered in CONTEXTS and previous != ".":
            yield lowered
        previous = token


def early_env_blocks(source):
    """Locate block-style workflow/job env; ignore step env and run bodies."""
    lines = source.splitlines()
    parents = []
    for index, line in enumerate(lines):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        while parents and parents[-1][0] >= indent:
            parents.pop()
        match = KEY.fullmatch(line)
        if not match:
            continue
        key = match["key"]
        path = [name for _, name in parents] + [key]
        if path == ["env"] or (len(path) == 3 and path[0] == "jobs" and key == "env"):
            end = index + 1
            while end < len(lines):
                nested = lines[end]
                if nested.strip() and not nested.lstrip().startswith("#"):
                    nested_indent = len(nested) - len(nested.lstrip(" "))
                    if nested_indent <= indent:
                        break
                end += 1
            yield index + 1, ".".join(path), "\n".join(lines[index:end])
        # Only mapping-valued entries are structural ancestors. In particular,
        # shell heredocs in a run block must not create a false workflow env.
        value = (match["value"] or "").split("#", 1)[0].strip()
        if not value:
            parents.append((indent, key))


def check(source):
    errors = []
    # Narrow YAML lexical boundary for root/job display names. Full YAML parsing
    # remains separate; do not interpret shell bodies or pretend this is a parser.
    for number, row in enumerate(source.splitlines(), 1):
        match = re.fullmatch(r"(?:name|    name):\s+(.+)", row)
        if match:
            value = match[1].split(" #", 1)[0].strip()
            if value and value[0] not in "\"'|>" and re.search(r":(?:\s|$)", value):
                errors.append(f"{number}: workflow/job name containing colon-space must be quoted")
    for line, path, block in early_env_blocks(source):
        allowed = WORKFLOW_ENV if path == "env" else JOB_ENV
        # Comments aren't workflow expressions. Values in this repository do
        # not contain literal '#'; keep the guard independent of a YAML parser.
        block = "\n".join(row.split(" #", 1)[0] for row in block.splitlines()
                          if not row.lstrip().startswith("#"))
        for match in EXPRESSION.finditer(block):
            number = line + block[:match.start()].count("\n")
            for context in sorted(set(expression_contexts(match[1])) - allowed):
                errors.append(f"{number}: {path} cannot use the {context} context; initialize runner values in a step")
            if re.search(r"\bhashFiles\s*\(", match[1], re.IGNORECASE):
                errors.append(f"{number}: {path} cannot use hashFiles")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path)
    arguments = parser.parse_args()
    paths = arguments.paths or sorted((ROOT / ".github/workflows").glob("*.y*ml"))
    failures = []
    for path in paths:
        failures.extend(f"{path}:{error}" for error in check(path.read_text(encoding="utf-8")))
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"Verified early env context availability in {len(paths)} workflow(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
