#!/usr/bin/env python3
"""Show progress, balances, and inventory from a saved Hay Day server farm."""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_HOME = ROOT / "data" / "9" / "home.json"
STARTER_TABLES = ROOT / "resources" / "starter" / "tables.json"
MONEY_IDS = ROOT / "resources" / "money-ids.json"
EXP_LEVELS = ROOT / "resources" / "exp-levels.csv"

def read_names() -> dict[int, str]:
    names: dict[int, str] = {}
    tables = json.loads(STARTER_TABLES.read_text())
    for table in tables:
        kind = table.get("kind")
        for index, row in enumerate(table.get("rows", [])):
            name = row.get("Name")
            if name:
                names[kind * 100_000 + index] = name
    money = json.loads(MONEY_IDS.read_text())
    for name, data_id in money.items():
        names[int(data_id)] = name
    return names


def level_threshold(level: int) -> int | None:
    with EXP_LEVELS.open(newline="") as f:
        for row in csv.DictReader(f):
            if int(row["Level"]) == level:
                return int(row["ExpToNextLevel"])
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("home", nargs="?", type=Path, default=DEFAULT_HOME,
                        help="saved home.json (default: %(default)s)")
    args = parser.parse_args()

    try:
        home = json.loads(args.home.read_text())
        stats = home.get("avatar_stats", {})
        names = read_names()
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        print(f"Could not read farm data: {exc}", file=sys.stderr)
        return 1

    def amount(data_id: str) -> int:
        return int(stats.get(data_id, 0))

    cash, free_cash = amount("1900000"), amount("1900131")
    level, xp = amount("1900002"), amount("1900003")
    needed = level_threshold(level)

    print(f"Saved farm: {args.home}")
    print("\nProgress")
    print(f"  Level:       {level}")
    if needed is None:
        print(f"  XP progress: {xp} (threshold unavailable)")
    else:
        print(f"  XP progress: {xp}/{needed}")

    print("\nBalances")
    print(f"  Cash:        {cash}")
    print(f"  Free cash:   {free_cash}")
    print(f"  Total gold:  {cash + free_cash}")
    print(f"  Diamonds:    {amount('1900001')}")

    print("\nInventory")
    nonzero_items: list[tuple[str, int]] = []
    for data_id, quantity in stats.items():
        item_id = int(data_id)
        # Kind 19 holds balances, level and XP (shown above); everything else is an item.
        if item_id // 100_000 != 19:
            quantity = int(quantity)
            if quantity:
                nonzero_items.append((names.get(item_id, f"Unknown item ({item_id})"), quantity))
    for name, quantity in sorted(nonzero_items, key=lambda item: item[0].casefold()):
        print(f"  {name}: {quantity}")
    if not nonzero_items:
        print("  (no items saved)")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
