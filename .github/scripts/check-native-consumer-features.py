#!/usr/bin/env python3
"""Reject unused native capabilities in each real consumer's selected graph.

This is not a package-retirement or binary-size assertion. Foundation tests may
still explicitly select rustix/all-apis and http-routing/form. Product filesystem
access uses fs/process, not mount syscalls; server extraction uses JSON and Query,
not Form. Query retains form-codec and URL form parsing independently of Form.
"""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
TARGET = "x86_64-unknown-linux-gnu"
CONSUMERS = ("storage-client", "storage-server", "storage-engine", "storage-rescue",
             "platform-io", "perf-check", "browser-smoke")
EDGES = ("normal,build", "normal,build,dev")


def validate(output, consumer, edges):
    selected = {}
    for line in output.splitlines():
        match = re.match(r"^(rustix|http-routing|storage-engine) v[^|]+\|([^\s]*)", line)
        if match:
            selected.setdefault(match[1], []).append(set(filter(None, match[2].split(","))))
    required = {"rustix"}
    if consumer in {"storage-server", "browser-smoke"} or (
            consumer == "storage-rescue" and edges == "normal,build,dev"):
        required.add("http-routing")
    if consumer in {"storage-engine", "storage-server", "storage-rescue", "perf-check", "browser-smoke"}:
        required.add("storage-engine")
    missing = required - selected.keys()
    if missing:
        raise AssertionError(f"{consumer} ({edges}): missing expected graph entries: {sorted(missing)}")
    for package, feature_sets in selected.items():
        if package == "storage-engine":
            expected = edges == "normal,build,dev" and consumer in {"storage-engine", "storage-server"}
            if edges == "normal,build" and any(
                    {"fault-injection", "test-support"} & features for features in feature_sets):
                raise AssertionError(f"{consumer}: production graph enables test-only engine features")
            if expected and not all("fault-injection" in features for features in feature_sets):
                raise AssertionError(f"{consumer}: test graph lacks fault-injection coverage")
            continue
        needed, forbidden = ({"fs", "process"}, {"mount"}) if package == "rustix" else (
            {"json", "query"}, {"form"})
        if any(not needed <= features or forbidden & features for features in feature_sets):
            raise AssertionError(f"{consumer} ({edges}): unexpected {package} capabilities: {feature_sets}")


def check(root):
    for consumer in CONSUMERS:
        for edges in EDGES:
            result = subprocess.run(
                ["cargo", "tree", "--locked", "--offline", "-p", consumer,
                 "--target", TARGET, "--edges", edges, "--prefix", "none", "--format", "{p}|{f}"],
                cwd=root, text=True, capture_output=True, check=True, timeout=120,
            )
            validate(result.stdout, consumer, edges)
    print(f"native consumer capabilities: {len(CONSUMERS) * len(EDGES)} separate production/dev graphs checked")


if __name__ == "__main__":
    check(ROOT)
