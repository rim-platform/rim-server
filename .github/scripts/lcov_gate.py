#!/usr/bin/env python3

import sys


def die(msg: str) -> None:
    print(msg)
    sys.exit(2)


def parse_lcov_totals(path: str):
    lf = 0
    lh = 0
    bf = 0
    bh = 0
    saw_lines = False
    saw_branches = False
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line.startswith("LF:"):
                lf += int(line[3:])
                saw_lines = True
            elif line.startswith("LH:"):
                lh += int(line[3:])
                saw_lines = True
            elif line.startswith("BRF:"):
                bf += int(line[4:])
                saw_branches = True
            elif line.startswith("BRH:"):
                bh += int(line[4:])
                saw_branches = True

    if not saw_lines:
        die(f"Invalid LCOV: missing LF/LH totals in {path}")
    return lf, lh, bf, bh


def pct(hit: int, found: int) -> float:
    if found == 0:
        return 100.0
    return (hit * 100.0) / found


def main(argv):
    if len(argv) != 4:
        die("usage: lcov_gate.py <lcov.info> <min_line_pct> <min_branch_pct>")

    path = argv[1]
    min_line = float(argv[2])
    min_branch = float(argv[3])

    lf, lh, bf, bh = parse_lcov_totals(path)
    line_pct = pct(lh, lf)
    branch_pct = pct(bh, bf)

    print(
        f"LCOV totals: lines {lh}/{lf} = {line_pct:.2f}%, branches {bh}/{bf} = {branch_pct:.2f}%"
    )

    ok = True
    if line_pct + 1e-9 < min_line:
        print(f"FAIL: line coverage {line_pct:.2f}% < {min_line:.2f}%")
        ok = False
    if branch_pct + 1e-9 < min_branch:
        print(f"FAIL: branch coverage {branch_pct:.2f}% < {min_branch:.2f}%")
        ok = False

    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main(sys.argv)
