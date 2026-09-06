#!/usr/bin/env python3
"""Measure a self-contained custom-section package, not executable DSL lowering.

Only Python's standard library is needed. The artifact table stores whole WASM
components once; call sites reference table indices. --self-test validates the
codec independently of the real agent bundle. Never load native serialized code.
"""
import argparse
import gzip
import hashlib
import json
import struct
from pathlib import Path

HEADER = b"\0asm\x0d\0\x01\0"
SECTION = b"runtara:isolated-package-research-v0"


def leb(value):
    result = bytearray()
    while True:
        byte = value & 127
        value >>= 7
        result.append(byte | (128 if value else 0))
        if not value:
            return bytes(result)


def read_leb(data, offset):
    result = 0
    for shift in range(0, 35, 7):
        byte = data[offset]
        offset += 1
        result |= (byte & 127) << shift
        if byte < 128:
            return result, offset
    raise ValueError("invalid u32 LEB")


def build(assets, calls, deduplicate=True):
    entries, sites, bodies, indices = [], [], [], {}
    offset = 0
    for name in calls:
        body = assets[name]
        digest = hashlib.sha256(body).hexdigest()
        if deduplicate and digest in indices:
            sites.append(indices[digest])
            continue
        sites.append(len(entries))
        indices[digest] = len(entries)
        entries.append({"sha256": digest, "offset": offset, "length": len(body)})
        bodies.append(body)
        offset += len(body)
    index = json.dumps({"artifacts": entries, "calls": sites}, separators=(",", ":")).encode()
    payload = leb(len(SECTION)) + SECTION + struct.pack("<I", len(index)) + index + b"".join(bodies)
    return HEADER + b"\0" + leb(len(payload)) + payload


def unpack(package):
    if package[:8] != HEADER or package[8] != 0:
        raise ValueError("invalid package header")
    length, cursor = read_leb(package, 9)
    if cursor + length != len(package):
        raise ValueError("invalid section length")
    name_length, cursor = read_leb(package, cursor)
    if package[cursor:cursor + name_length] != SECTION:
        raise ValueError("wrong section")
    cursor += name_length
    index_length = struct.unpack_from("<I", package, cursor)[0]
    cursor += 4
    index = json.loads(package[cursor:cursor + index_length])
    cursor += index_length
    blobs = package[cursor:]
    for entry in index["artifacts"]:
        start, size = entry["offset"], entry["length"]
        if start < 0 or size < 0 or start + size > len(blobs):
            raise ValueError("out of bounds artifact")
        if hashlib.sha256(blobs[start:start + size]).hexdigest() != entry["sha256"]:
            raise ValueError("artifact digest mismatch")
    if any(i < 0 or i >= len(index["artifacts"]) for i in index["calls"]):
        raise ValueError("out of bounds call")
    return index


def self_test():
    assets = {"a": HEADER, "alias": HEADER, "b": HEADER + b"\0\x02\x01x"}
    calls = ["a", "alias", "b", "a"]
    package = build(assets, calls)
    index = unpack(package)
    assert len(index["artifacts"]) == 2 and index["calls"] == [0, 0, 1, 0]
    assert len(unpack(build(assets, calls, False))["artifacts"]) == 4
    for bad in [package[:-1], package[:-1] + bytes([package[-1] ^ 1])]:
        try:
            unpack(bad)
        except ValueError:
            pass
        else:
            raise AssertionError("damaged package was accepted")
    print("Package codec: deduplication, references, truncation and tampering checks passed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--components", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
    if args.components is None:
        if not args.self_test:
            parser.error("--components or --self-test is required")
        return
    files = sorted(args.components.glob("runtara_agent_*.wasm"))
    files += sorted(args.components.glob("runtara_workflow_*.wasm"))
    assets = {p.name: p.read_bytes() for p in files}
    if not assets or "runtara_agent_http.wasm" not in assets:
        parser.error("a staged component bundle with the HTTP agent is required")
    cases = []
    for name, calls in [(f"http_{n}_calls", ["runtara_agent_http.wasm"] * n) for n in [1, 10, 100]] + [("all_staged_components_once", list(assets))]:
        case = {"name": name, "call_sites": len(calls)}
        for mode, dedup in [("flat_deduplicated", True), ("naive_per_call_copy", False)]:
            package = build(assets, calls, dedup)
            index = unpack(package)
            case[mode] = {"raw_bytes": len(package), "gzip_bytes": len(gzip.compress(package, mtime=0)), "embedded_artifacts": len(index["artifacts"])}
        cases.append(case)
    report = {
        "format": "research-only WASM custom-section container; no workflow code or executable loader",
        "deduplication": "whole artifact SHA-256; no extraction of shared internals",
        "assets": [{"name": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in assets.items()],
        "cases": cases,
    }
    rendered = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered)
    else:
        print(rendered, end="")


if __name__ == "__main__":
    main()
