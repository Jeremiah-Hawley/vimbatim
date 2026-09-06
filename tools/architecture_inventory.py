#!/usr/bin/env python3
"""Print a small, dependency-free inventory of Rust source files."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def main() -> None:
    rows = []
    for path in sorted((ROOT / "src").glob("*.rs")):
        lines = sum(1 for _ in path.open(encoding="utf-8"))
        rows.append((lines, path.relative_to(ROOT)))

    print("# Architecture Inventory\n")
    print(f"{len(rows)} Rust files; {sum(lines for lines, _ in rows):,} lines.\n")
    print("| Lines | File |\n|---:|---|")
    for lines, path in sorted(rows, reverse=True):
        print(f"| {lines:,} | `{path}` |")


if __name__ == "__main__":
    main()
