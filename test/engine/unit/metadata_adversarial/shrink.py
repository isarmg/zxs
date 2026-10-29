#!/usr/bin/env python3
"""Drop unreachable objects from a captured regression input.

Every matching digest string conservatively retains all domains at that digest.
This is a corpus reducer, not a parser or legality oracle: the resulting input
must still be replayed through the real Rust admission entry.
"""

import argparse
import json
from pathlib import Path


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    captured = json.loads(args.input.read_text())
    objects = captured["objects"]
    addresses = {}
    for address in objects:
        kind, digest = address.split("/", 1)
        if not kind or len(digest) != 64:
            raise ValueError(f"invalid captured object address: {address}")
        addresses.setdefault(digest, []).append(address)
    retained = set()
    pending = list(strings(captured["snapshot"]))
    while pending:
        for address in addresses.get(pending.pop(), []):
            if address not in retained:
                retained.add(address)
                pending.extend(strings(objects[address]))
    captured["objects"] = {address: objects[address] for address in sorted(retained)}
    args.output.write_text(json.dumps(captured, ensure_ascii=False, sort_keys=True,
                                      separators=(",", ":")) + "\n")
    print(json.dumps({"before_objects": len(objects), "after_objects": len(retained),
                      "status": "requires_real_entry_replay"}))


if __name__ == "__main__":
    main()
