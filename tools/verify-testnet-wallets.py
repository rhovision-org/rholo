#!/usr/bin/env python3
"""Fail closed when a public-testnet genesis wallet amount uses REV instead of drops."""
import argparse
from pathlib import Path

DROPS_PER_REV = 100_000_000


def check_wallets(text: str, expected_rev: int) -> list[str]:
    if expected_rev <= 0:
        return ["expected REV allocation must be positive"]
    expected = expected_rev * DROPS_PER_REV
    errors: list[str] = []
    seen: set[str] = set()
    accounts = 0
    for line_number, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        fields = [field.strip() for field in line.split(",")]
        if len(fields) != 2 or not fields[0]:
            errors.append(f"line {line_number}: expected address,drops")
            continue
        address, amount = fields
        if address in seen:
            errors.append(f"line {line_number}: duplicate wallet address")
        seen.add(address)
        accounts += 1
        if not amount.isdecimal():
            errors.append(f"line {line_number}: amount is not an unsigned integer in drops")
            continue
        drops = int(amount)
        if drops < expected:
            errors.append(
                f"line {line_number}: {drops} drops is below {expected} drops "
                f"({expected_rev} REV); genesis amounts are drops, not REV"
            )
    if accounts == 0:
        errors.append("no funded wallets were found")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wallets", type=Path, help="genesis wallets.txt (address,drops)")
    parser.add_argument("--minimum-rev", type=int, default=10_000)
    args = parser.parse_args()
    errors = check_wallets(args.wallets.read_text(encoding="utf-8"), args.minimum_rev)
    if errors:
        for error in errors:
            print("FAIL:", error)
        return 1
    print(f"PASS: all genesis wallet entries have at least {args.minimum_rev} REV")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
