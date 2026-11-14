"""Compares the failed checks of a run on tflip with a run on the original.

    python3 -I compare.py <work folder>

Reads results.txt and logs/ (tflip) and sap/results.txt and sap/logs/ (the
original), as run.sh --sap writes them, and prints, per test file, the
checks that fail on only one engine or on both with other values. A check
is a source line; its "values" line is doctest's. Ends with the totals:

    same      the check fails on both with the same values: the test is wrong
    differ    it fails on both, with other values
    tflip     it fails on tflip only
    original  it fails on the original only: tflip agrees with OpenADS there,
              or the original stopped at an earlier REQUIRE
"""
import os
import re
import sys

ERROR = re.compile(r'/(\w+\.cpp):(\d+): (?:FATAL )?ERROR: (.*)')


def failures(path):
    found = {}
    if not os.path.exists(path):
        return found
    with open(path, errors='replace') as f:
        lines = f.read().splitlines()
    for i, line in enumerate(lines):
        m = ERROR.search(line)
        if m:
            nxt = lines[i + 1].strip() if i + 1 < len(lines) else ''
            found.setdefault(int(m.group(2)), nxt if nxt.startswith('values:') else m.group(3)[:100])
    return found


def results(path):
    with open(path) as f:
        return dict(line.split() for line in f if line.strip())


def main(work):
    tflip = results(os.path.join(work, 'results.txt'))
    sap = results(os.path.join(work, 'sap', 'results.txt'))
    totals = dict(same=0, differ=0, tflip=0, original=0)
    for name in sorted(tflip):
        t = failures(os.path.join(work, 'logs', name + '.log'))
        s = failures(os.path.join(work, 'sap', 'logs', name + '.log'))
        same = [k for k in t.keys() & s.keys() if t[k] == s[k]]
        differ = sorted(k for k in t.keys() & s.keys() if t[k] != s[k])
        only_t = sorted(t.keys() - s.keys())
        only_s = sorted(s.keys() - t.keys())
        for key, n in (('same', len(same)), ('differ', len(differ)),
                       ('tflip', len(only_t)), ('original', len(only_s))):
            totals[key] += n
        print(f'{name}  tflip {tflip[name]}  original {sap.get(name, "-")}  '
              f'same {len(same)}')
        for k in differ:
            print(f'    {k}  differ    tflip {t[k]}  original {s[k]}')
        for k in only_t:
            print(f'    {k}  tflip     {t[k]}')
        for k in only_s:
            print(f'    {k}  original  {s[k]}')
    print('checks failing: ' + ', '.join(f'{k} {v}' for k, v in totals.items()))


if __name__ == '__main__':
    main(sys.argv[1])
