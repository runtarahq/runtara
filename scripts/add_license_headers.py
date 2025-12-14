#!/usr/bin/env python3
"""
Add the required license header to all Rust source files in the repository.

Usage:
  python scripts/add_license_headers.py

This script:
- Scans tracked and untracked (non-ignored) *.rs files.
- Prepends the license header when it is not already present.
- Leaves files untouched if the header already exists.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


HEADER_LINES = [
    "// Copyright (C) 2025 SyncMyOrders Sp. z o.o.",
    "// SPDX-License-Identifier: AGPL-3.0-or-later",
    "",
]
HEADER = "\n".join(HEADER_LINES)


def get_repo_root() -> Path:
    try:
        output = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], text=True
        )
        return Path(output.strip())
    except Exception:
        return Path(__file__).resolve().parent.parent


def rust_files(repo_root: Path) -> list[Path]:
    result = subprocess.run(
        [
            "git",
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*.rs",
        ],
        cwd=repo_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=True,
    )
    return [repo_root / line for line in result.stdout.splitlines() if line.strip()]


def has_header(body: str) -> bool:
    # Allow optional leading newlines (but nothing else) before the header.
    return body.startswith(HEADER) or body.lstrip("\n").startswith(HEADER)


def add_header(path: Path) -> bool:
    original = path.read_text(encoding="utf-8")

    bom = "\ufeff" if original.startswith("\ufeff") else ""
    body = original[len(bom) :] if bom else original

    if has_header(body):
        return False

    updated = f"{bom}{HEADER}{body}"
    path.write_text(updated, encoding="utf-8")
    return True


def main() -> int:
    repo_root = get_repo_root()
    rust_paths = rust_files(repo_root)

    if not rust_paths:
        print("No Rust files found.")
        return 0

    updated_count = 0
    for rust_file in rust_paths:
        if add_header(rust_file):
            updated_count += 1

    print(f"Processed {len(rust_paths)} Rust files; added header to {updated_count}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
