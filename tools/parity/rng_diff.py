#!/usr/bin/env python3
"""Compare the RNG draw logs that vanilla and Pumpkin write under the lockdown (--rng-seed).

Each line is `<owner>\\t<method>\\t<arg>\\t<result>`. Owners draw from their own streams, so only
the order within one owner matters: this groups both logs by owner and reports, per owner, how
many draws match and the first one that doesn't.

    python tools/parity/rng_diff.py <vanilla rng.log> <pumpkin rng.log>
"""

import argparse
import sys
from collections import defaultdict
from pathlib import Path

# Written when `/tick freeze` restarts every stream; only draws after the last one are compared.
RESET_MARK = "#reset"
# How many owners that match fully are listed before the rest are summarised.
SHOW_MATCHING = 10


def read_log(path):
    """Draws per owner after the last reset mark, in order, as (line number, method, arg, result)."""
    draws = defaultdict(list)
    if not path.exists():
        return None
    with open(path, encoding="utf8", errors="replace") as log:
        for number, line in enumerate(log, 1):
            if line.startswith(RESET_MARK):
                draws.clear()
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 4:
                continue
            owner, method, arg, result = parts
            draws[owner].append((number, method, arg, result))
    return draws


def compare(vanilla, pumpkin):
    """Per owner: (matching draws, first divergent index or None, vanilla draw, pumpkin draw).

    The index is None when the draws match, and also when one log only goes on longer: the servers
    keep ticking for a moment after a trace ends, so one of them often gets a few more ticks in.
    """
    results = {}
    for owner in sorted(set(vanilla) & set(pumpkin)):
        v, p = vanilla[owner], pumpkin[owner]
        matching = 0
        for a, b in zip(v, p):
            if a[1:] != b[1:]:
                break
            matching += 1
        if matching == min(len(v), len(p)):
            results[owner] = (matching, None, None, None)
        else:
            results[owner] = (matching, matching,
                              v[matching] if matching < len(v) else None,
                              p[matching] if matching < len(p) else None)
    return results


def fmt(draw):
    if draw is None:
        return "<no more draws>"
    number, method, arg, result = draw
    return f"{method}({'' if arg == '-' else arg}) = {result}  [line {number}]"


def report(vanilla_path, pumpkin_path):
    """Prints the comparison; returns the number of owners whose draws differ or are one-sided."""
    vanilla, pumpkin = read_log(Path(vanilla_path)), read_log(Path(pumpkin_path))
    if vanilla is None or pumpkin is None:
        missing = [str(p) for p, d in ((vanilla_path, vanilla), (pumpkin_path, pumpkin)) if d is None]
        print(f"RNG: no draw log at {', '.join(missing)}")
        return 1
    results = compare(vanilla, pumpkin)
    same = [o for o, r in results.items() if r[1] is None and len(vanilla[o]) == len(pumpkin[o])]
    longer = [o for o, r in results.items() if r[1] is None and len(vanilla[o]) != len(pumpkin[o])]
    differ = [o for o, r in results.items() if r[1] is not None]
    only_vanilla = sorted(set(vanilla) - set(pumpkin))
    only_pumpkin = sorted(set(pumpkin) - set(vanilla))
    print(f"RNG: {len(same)} owners match, {len(longer)} match until one log ends, {len(differ)} differ, "
          f"{len(only_vanilla)} only on vanilla, {len(only_pumpkin)} only on Pumpkin")
    for owner in differ:
        matching, index, v, p = results[owner]
        print(f"DIFF {owner}: {matching} draws match (vanilla {len(vanilla[owner])}, "
              f"Pumpkin {len(pumpkin[owner])}); draw {index}:")
        print(f"       vanilla: {fmt(v)}")
        print(f"       pumpkin: {fmt(p)}")
    for owner in same[:SHOW_MATCHING]:
        print(f"ok   {owner}: {results[owner][0]} draws")
    if len(same) > SHOW_MATCHING:
        print(f"ok   ... {len(same) - SHOW_MATCHING} more matching owners")
    for side, owners, draws in (("vanilla", only_vanilla, vanilla), ("Pumpkin", only_pumpkin, pumpkin)):
        for owner in owners:
            print(f"ONLY {side} {owner}: {len(draws[owner])} draws, first {fmt(draws[owner][0])}")
    return len(differ) + len(only_vanilla) + len(only_pumpkin)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("vanilla", type=Path, help="vanilla rng.log")
    parser.add_argument("pumpkin", type=Path, help="Pumpkin rng.log")
    args = parser.parse_args()
    sys.exit(1 if report(args.vanilla, args.pumpkin) else 0)


if __name__ == "__main__":
    main()
