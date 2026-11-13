#!/usr/bin/env python3
"""Prints, as JSON, the byte ranges in which the files of two folders differ:
{file name: [[offset, length], ...]}, for run.sh's volatile.json. A file that
exists in only one of them, or whose sizes differ, is an error.

    tools/ace/write/volatile.py <folder> <folder>
    tools/ace/write/volatile.py --snaps <snaps> <snaps-again>

With --snaps, the folders hold a snapshot each (run.sh's snaps/<name>), and
the files are named <name>/<file>.
"""
import json
import os
import sys


def ranges(a, b):
    out = []
    i = 0
    while i < len(a):
        if a[i] == b[i]:
            i += 1
            continue
        start = i
        while i < len(a) and a[i] != b[i]:
            i += 1
        out.append([start, i - start])
    return out


def differing(first, second):
    names = sorted(os.listdir(first))
    if names != sorted(os.listdir(second)):
        sys.exit("the folders hold different files")
    result = {}
    for name in names:
        with open(os.path.join(first, name), "rb") as f:
            a = f.read()
        with open(os.path.join(second, name), "rb") as f:
            b = f.read()
        if len(a) != len(b):
            sys.exit("%s: %d and %d bytes" % (name, len(a), len(b)))
        found = ranges(a, b)
        if found:
            result[name] = found
    return result


def main(args):
    if args[0] == "--snaps":
        first, second = args[1:]
        result = {}
        for snap in sorted(os.listdir(first)):
            found = differing(os.path.join(first, snap), os.path.join(second, snap))
            for name, ranges_ in found.items():
                result[snap + "/" + name] = ranges_
    else:
        result = differing(*args)
    json.dump(result, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main(sys.argv[1:])
