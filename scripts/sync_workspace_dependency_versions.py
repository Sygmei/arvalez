#!/usr/bin/env python3

from __future__ import annotations

import pathlib
import re
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parent.parent
CARGO_TOML = ROOT / "Cargo.toml"

INTERNAL_CRATES = [
    "arvalez-cli",
    "arvalez-ir",
    "arvalez-openapi",
    "arvalez-plugin-runtime",
    "arvalez-plugin-sdk",
    "arvalez-target-go",
    "arvalez-target-python",
    "arvalez-target-typescript",
]


def main() -> int:
    data = tomllib.loads(CARGO_TOML.read_text())
    workspace_version = data["workspace"]["package"]["version"]
    raw = CARGO_TOML.read_text()
    changed = False

    for crate in INTERNAL_CRATES:
        pattern = rf'({re.escape(crate)}\s*=\s*\{{\s*version\s*=\s*")[^"]+(".*\}})'
        updated, count = re.subn(pattern, rf'\g<1>{workspace_version}\g<2>', raw)
        if count != 1:
            raise SystemExit(f"Failed to update workspace dependency version for {crate}")
        if updated != raw:
            changed = True
            raw = updated

    if changed:
        CARGO_TOML.write_text(raw)
        print(f"Updated workspace dependency versions to {workspace_version} in {CARGO_TOML.name}.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
