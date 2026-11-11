#!/usr/bin/env python3
"""Rank the names the server could not resolve, most frequent first.

Reads the server's JSON logs (files given as arguments, or stdin) and counts
the miss events: every `not_found` and `ambiguous` verdict logs one.

    hundred serve --index index/full > server.log
    scripts/rank_misses.py server.log | head -50
    scripts/rank_misses.py --reason not_found server.log

Output is tab separated: count, reason, normalized name.
"""

import argparse
import collections
import json
import sys

MISS_TARGET = "hundred::miss"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("logs", nargs="*", help="log files (default: stdin)")
    parser.add_argument("--reason", choices=["not_found", "ambiguous"], help="only this reason")
    parser.add_argument("--min-count", type=int, default=1, help="hide names seen fewer times")
    args = parser.parse_args()

    counts = collections.Counter()
    streams = [open(path, encoding="utf-8") for path in args.logs] or [sys.stdin]
    for stream in streams:
        for line in stream:
            if MISS_TARGET not in line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            fields = event.get("fields", {})
            if event.get("target") != MISS_TARGET or "normalized" not in fields:
                continue
            if args.reason and fields.get("reason") != args.reason:
                continue
            counts[(fields.get("reason", ""), fields["normalized"])] += 1

    for (reason, name), count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
        if count >= args.min_count:
            print(f"{count}\t{reason}\t{name}")


if __name__ == "__main__":
    main()
