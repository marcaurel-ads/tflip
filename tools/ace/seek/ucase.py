#!/usr/bin/env python3
"""Works out, from the original engine's keys, which character UPPER and LOWER
map each character of NChar text to: crates/tflip-storage/src/icu_case.txt.
See run_ucase.sh, which runs these steps with ucase.exe (ucase.c) under Wine.

    ucase.py strings <out> <first> <last>             every character in the range
    ucase.py table <icu_root.txt> <keys>...           > icu_case.txt

A strings file has a string per line, as UTF-16LE hex; a keys file is what
ucase.exe writes for one: "<string hex> <UPPER key hex> <LOWER key hex>".

The engine maps a character at a time (`UPPER` of ß is ß, of ΑΣ is ΑΣ, and
LOWER of it is ασ, not ας), by Unicode 5.1's table, the version of ICU 4.0,
the engine's. What `table` can see of a character's image is its key, so it
takes, among the characters with that key, Python's simple mapping when that
has it, else the lowest one, and says so where the key has more than one.
"""
import sys

import nkeys


def strings(out, first, last):
    nkeys.write(out, [chr(c) for c in range(first, last + 1) if not 0xD800 <= c < 0xE000])


def simple(c, kind):
    mapped = c.upper() if kind == 'upper' else c.lower()
    return mapped if len(mapped) == 1 else c


def table(root, paths):
    tab, compressible, unsupported = nkeys.load_table(root)
    seen = {}
    for path in paths:
        for line in open(path):
            words = line.split()
            if len(words) != 3 or 'rc' in words[1:]:
                continue
            seen[bytes.fromhex(words[0]).decode('utf-16-le')] = (
                bytes.fromhex(words[1]).rstrip(b'\0'), bytes.fromhex(words[2]).rstrip(b'\0'))
    by_key = {}
    own = {}
    for c in seen:
        k = nkeys.key(c.ljust(4), tab, compressible, unsupported)
        own[c] = k
        if k is not None:
            by_key.setdefault(k.rstrip(b'\0'), []).append(c)
    print('# What the original engine\'s UPPER and LOWER make of each NChar character, one')
    print('# that maps to another or to one that sorts the same: code point, image of UPPER')
    print('# (- for itself), image of LOWER. Worked out from its keys by')
    print('# tools/ace/seek/ucase.py, which says how. A `?` is a character whose key')
    print('# tflip cannot tell from the others, with its key.')
    for c in sorted(seen):
        images = []
        for observed, kind in zip(seen[c], ('upper', 'lower')):
            if own[c] is not None and observed == own[c].rstrip(b'\0'):
                images.append('-')
                continue
            # No key to compare with: a character both functions leave alone.
            if own[c] is None and seen[c][0] == seen[c][1]:
                images.append('-')
                continue
            options = by_key.get(observed, [])
            guess = simple(c, kind)
            if guess in options:
                chosen = guess
            elif options:
                chosen = options[0]
            else:
                images.append('?' + observed.hex())
                continue
            images.append('%04X' % ord(chosen) if len(chosen) == 1 else '?')
        if images != ['-', '-']:
            print('%04X %s %s' % (ord(c), images[0], images[1]))


def main():
    if sys.argv[1] == 'strings':
        strings(sys.argv[2], int(sys.argv[3], 16), int(sys.argv[4], 16))
    elif sys.argv[1] == 'table':
        table(sys.argv[2], sys.argv[3:])
    else:
        sys.exit(__doc__)


if __name__ == '__main__':
    main()
