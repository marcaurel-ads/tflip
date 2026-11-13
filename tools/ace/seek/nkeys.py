#!/usr/bin/env python3
"""Works out, from the original engine's keys, the table tflip builds NChar and
NVarChar keys (ICU sort keys, `:en_US`) from: crates/tflip-storage/src/icu_root.txt.
See run_nkeys.sh, which runs these steps with nkeys.exe (nkeys.c) under Wine.

    nkeys.py singles <out>                          every BMP character
    nkeys.py astral <out>                           the supplementary ones taken
    nkeys.py contractions <allkeys.txt> <out>       those of UCA 5.1.0
    nkeys.py pairs <out> <keys>...                  XX per lead byte
    nkeys.py follow <out> <keys>...                 each candidate, then "äB"
    nkeys.py table <pairs.keys> <follow.keys> <keys>... > icu_root.txt
    nkeys.py random <icu_root.txt> <seed> <count> <out>
    nkeys.py check <icu_root.txt> <keys> <field length>
    nkeys.py mismatches <icu_root.txt> <field length> <keys>...

A strings file has a string per line, as UTF-16LE hex; a keys file is what
nkeys.exe writes for one: "<string hex> <key hex>".

The model. A key is the primary weights, 0x01, the secondary weights, 0x01,
the tertiary weights (ICU's sort key at tertiary strength). Each character
stands for a list of collation elements: their primaries (1, 2 or 4 bytes,
by the lead byte), and the secondary and tertiary weights, where the common
weight 0x05 is written compressed: a run of them before a higher weight as
one byte counting down from 0x86 (secondary; 0x85 tertiary), at the end
counting up from 0x05, in chunks of 0x40 (secondary; tertiary 0x54 down and
0x2B up). Consecutive primaries with the same lead byte, if that lead is a
compressible one, are written without it after the first; a change of lead
after such a run is marked by 0xFF (to a higher lead) or 0x03 (lower). So a
character's elements come out of the engine's key of it alone (and of it
followed by "äB", which tells a byte that is a weight of its own from a run
of commons), and a string's key is its characters' elements, concatenated
and written so. Checked against random strings (`check`).

What the table leaves out is weighed by formulas: ideographs and characters no
element is defined for get ICU's implicit weights (`implicit`), Hangul
syllables weigh as their jamo, noncharacters have no weight, and a Thai or Lao
vowel written before its consonant sorts after it (a rule of `elements`, the
contractions of UCA 5.1.0 that are not that are in the table). A character
whose elements could not be told is marked `?` and fails."""
import collections
import itertools
import random
import sys
import unicodedata

# The blocks the table takes characters from: the whole BMP but the ideographs
# and Hangul syllables (weighed by formula, see crates/tflip-storage/src/icu.rs)
# and the private use area.
RANGES = [(0x0000, 0x33FF), (0x4DC0, 0x4DFF), (0xA000, 0xABFF), (0xD7A4, 0xD7FF), (0xF900, 0xFAFF), (0xFB00, 0xFFFF)]

# Supplementary characters the table takes too: plane 1 (scripts, symbols), the
# compatibility ideographs and the tag and variation characters.
ASTRAL = [(0x10000, 0x1FFFF), (0x2F800, 0x2FA1F), (0xE0000, 0xE01FF)]

SEC = dict(top=0x86, bottom_count=0x40, top_count=0x40)
TER = dict(top=0x85, bottom_count=0x2B, top_count=0x54)
THAI = set(range(0x0E01, 0x0E2F))
LAO = ({0x0E81, 0x0E82, 0x0E84, 0x0E87, 0x0E88, 0x0E8A, 0x0E8D, 0x0EA5, 0x0EA7, 0x0EAA, 0x0EAB,
        0x0EAD, 0x0EAE, 0x0EDC, 0x0EDD} | set(range(0x0E94, 0x0E98)) | set(range(0x0E99, 0x0EA0))
       | set(range(0x0EA1, 0x0EA4)))
PREVOWEL_CONSONANTS = {
    0xE40: THAI, 0xE41: THAI, 0xE42: THAI, 0xE43: THAI, 0xE44: THAI,
    0xEC0: LAO, 0xEC1: LAO, 0xEC2: LAO, 0xEC3: LAO, 0xEC4: LAO,
}
PREVOWEL_TERTIARY = 0xBD
C = 0x05  # a compressible common weight


def wide(text):
    return text.encode('utf-16-le').hex()


def read(path):
    out = {}
    for line in open(path):
        t, k = line.split(maxsplit=1)
        if k.startswith('rc'):
            continue
        out[bytes.fromhex(t).decode('utf-16-le')] = bytes.fromhex(k.strip()).rstrip(b'\0')
    return out


def write(path, strings):
    with open(path, 'w') as f:
        f.write('\n'.join(wide(s) for s in strings) + '\n')


def compress(tokens, top, bottom_count, top_count):
    out = []
    count = 0

    def flush(up):
        nonlocal count
        if up:
            while count > top_count:
                out.append(top - top_count)
                count -= top_count
            out.append(top - (count - 1))
        else:
            while count > bottom_count:
                out.append(0x05 + bottom_count)
                count -= bottom_count
            out.append(0x05 + (count - 1))
        count = 0

    for t in tokens:
        if t == C:
            count += 1
            continue
        if count:
            flush(t > 0x05)
        out.append(t)
    if count:
        flush(False)
    return bytes(out)


def interpretations(level, top, bottom_count, top_count):
    """Every token list a level's bytes may stand for: each byte a weight of
    its own, or a run of commons."""
    options = []
    for b in level:
        o = [[b]]
        if 0x05 <= b <= 0x05 + bottom_count:
            o.append([C] * (b - 0x05 + 1))
        if top - top_count <= b <= top:
            o.append([C] * (top - b + 1))
        options.append(o)
    if sum(len(o) > 1 for o in options) > 10:
        return []
    return [sum(choice, []) for choice in itertools.product(*options)]


def compress_primary(ces, compressible):
    out = []
    lead = None
    for ce in ces:
        if lead is not None:
            if ce[0] == lead:
                out.extend(ce[1:])
                continue
            out.append(0xFF if ce[0] > lead else 0x03)
            lead = None
        out.extend(ce)
        if ce[0] in compressible:
            lead = ce[0]
    return bytes(out)


def parse_primary(p, lead_lens, compressible):
    """Every way the bytes of `p` split into primaries (each lead byte stands
    for primaries of some lengths), with the leads of a compressible run
    left out."""
    def go(i, lead):
        if i == len(p):
            yield []
            return
        b = p[i]
        if lead is not None:
            if b in (0x03, 0xFF):
                for rest in go(i + 1, None):
                    yield rest
                return
            for n in lead_lens.get(lead, ()):
                if i + n - 1 <= len(p):
                    ce = bytes([lead]) + p[i:i + n - 1]
                    for rest in go(i + n - 1, lead):
                        yield [ce] + rest
            return
        for n in lead_lens.get(b, ()):
            if i + n <= len(p):
                for rest in go(i + n, b if b in compressible else None):
                    yield [p[i:i + n]] + rest

    return list(go(0, None))


def single(s):
    """Whether a character has one collation element: not a contraction, nor a
    Hangul syllable (two or three jamo)."""
    return len(s) == 1 and not 0xAC00 <= ord(s) <= 0xD7A3


def lead_lengths(singles):
    """The lengths of the primaries each lead byte starts, from the characters
    of one element."""
    lens = collections.defaultdict(collections.Counter)
    for s, b in singles.items():
        p = b.split(b'\x01')
        if single(s) and len(p) == 3 and len(p[1]) == 1 and len(p[2]) == 1 and p[0]:
            lens[p[0][0]][len(p[0])] += 1
    # A length that most of the characters of a lead do not have may be that
    # of two elements.
    lead_lens = {lead: {n for n, c in counts.items() if c * 10 >= max(counts.values())}
                 for lead, counts in lens.items()}
    lead_lens[0x06] = {1}  # the space
    return lead_lens


def one_element(singles):
    """A character of one element per lead byte of a primary of 2+ bytes."""
    pick = {}
    for s, b in sorted(singles.items()):
        p = b.split(b'\x01')
        if single(s) and len(p) == 3 and len(p[1]) == 1 and len(p[2]) == 1 and len(p[0]) >= 2:
            pick.setdefault(p[0][0], s)
    return pick


def compressible_leads(singles, pairs):
    out = set()
    for lead, s in one_element(singles).items():
        p = singles[s].split(b'\x01')[0]
        k = pairs.get(s + s)
        if k is not None and k.split(b'\x01')[0] == p + p[1:]:
            out.add(lead)
    return out


def solve(level, after, suffix, params):
    """A character's tokens of a level, from its key's level and the level of
    its key followed by `suffix`'s tokens; None unless they tell."""
    found = set()
    for interp in interpretations(level, **params):
        if compress(interp, **params) == level and compress(interp + suffix, **params) == after:
            found.add(tuple(interp))
    if not found:
        return None
    # A byte that may be a run is one (weights of their own are above runs).
    best = max(found, key=lambda t: t.count(C))
    if sum(t.count(C) == best.count(C) for t in found) > 1:
        return None
    return list(best)


def read_all(paths):
    out = {}
    for path in paths:
        out.update(read(path))
    return out


def candidates(singles):
    """The strings the table has entries for: the characters of the ranges
    that are not unassigned (the formula below weighs those), and the
    contractions."""
    for s in sorted(singles):
        if len(s) > 1:
            yield s
            continue
        cp = ord(s)
        if cp == 0x20 or not any(lo <= cp <= hi for lo, hi in RANGES + ASTRAL):
            continue
        # The Hangul syllables' range runs on to D7AF.
        if unicodedata.category(s) in ('Cn', 'Co', 'Cs') and not 0xD7A4 <= cp <= 0xD7AF:
            continue
        yield s


def implicit(cp):
    """The primary an ideograph, or a character no collation element is
    defined for, gets: ICU's implicit weights, one 3-byte (the ideographs of
    the two main blocks) or 4 bytes, from the character's place in a
    sequence."""
    if 0x4E00 <= cp <= 0x9FFF or 0x3400 <= cp <= 0x4DBF:
        n = 1 + (cp - 0x4E00 if cp >= 0x4E00 else cp - 0x3400 + 0xA000 - 0x4E00 + 34)
        return bytes([0xE0 + n // (125 * 251), 4 + n // 125 % 251, 4 + 2 * (n % 125)])
    g = cp + 535635 if 0x20000 <= cp <= 0x2A6D6 else cp + 515729 + 2 * 567009
    out = [4 + 28 * (g % 9)]
    g //= 9
    out.append(4 + g % 251)
    g //= 251
    out.append(4 + g % 251)
    g //= 251
    out.append(0xE0 + g)
    return bytes(reversed(out))


def ignorable(cp):
    return 0xFDD0 <= cp <= 0xFDEF or cp & 0xFFFE == 0xFFFE


def build_table(singles, pairs, follow):
    lead_lens = lead_lengths(singles)
    compressible = compressible_leads(singles, pairs)
    known = set()
    for s, b in singles.items():
        p = b.split(b'\x01')
        if single(s) and len(p) == 3 and len(p[1]) == 1 and len(p[2]) == 1 and p[0]:
            known.add(p[0])
    def decode(s):
        k, q = singles[s], follow.get(s + 'äB')
        if q is None:
            return None
        p, q = k.split(b'\x01'), q.split(b'\x01')
        if len(p) != 3 or len(q) != 3:
            return None
        splits = [ces for ces in parse_primary(p[0], lead_lens, compressible)
                  if compress_primary(ces, compressible) == p[0]
                  and compress_primary(ces + [b'\x2c', b'\x2e'], compressible) == q[0]]
        # Primaries of characters of one element are the ones to expect.
        splits = [ces for ces in splits if all(ce in known for ce in ces)] or splits
        if len(splits) != 1:
            return None
        sec = solve(p[1], q[1], [C, 0x9D, C], SEC)
        ter = solve(p[2], q[2], [C, C, 0x8F], TER)
        if sec is None or ter is None:
            return None
        return splits[0], sec, ter

    tab = {}
    unsupported = set()
    for s in candidates(singles):
        entry = decode(s)
        if entry is None:
            unsupported.add(s)
        # What the formula gives anyway is left out.
        elif not (len(s) == 1 and entry == ([implicit(ord(s))], [C], [C])):
            tab[s] = entry
    # The space, whose own key the field's padding hides.
    tab[' '] = ([b'\x06'], [C], [C])
    return tab, compressible, unsupported


def load_table(path):
    tab = {}
    compressible = set()
    unsupported = set()
    for line in open(path):
        if line.startswith('#') or not line.strip():
            continue
        words = line.split()
        if words[0] == 'compressible':
            compressible = {int(w, 16) for w in words[1:]}
            continue
        text = ''.join(chr(int(cp, 16)) for cp in words[0].split('+'))
        if words[1] == '?':
            unsupported.add(text)
            continue
        part = lambda w: b'' if w == '-' else bytes.fromhex(w)
        ces = [] if words[1] == '-' else [bytes.fromhex(ce) for ce in words[1].split('.')]
        tab[text] = (ces, list(part(words[2])), list(part(words[3])))
    return tab, compressible, unsupported


def elements(text, tab, unsupported):
    """The collation elements of `text`, a list of (primaries, secondary
    weights, tertiary weights) per character or contraction; None for a lone
    surrogate."""
    out = []
    i = 0
    while i < len(text):
        for n in (3, 2):
            if text[i:i + n] in tab and len(text[i:i + n]) == n:
                out.append(tab[text[i:i + n]])
                i += n
                break
        else:
            c = text[i]
            cp = ord(c)
            if c in unsupported:
                return None
            nxt = text[i + 1] if i + 1 < len(text) else ''
            if nxt and ord(nxt) in PREVOWEL_CONSONANTS.get(cp, ()):
                # A Thai or Lao vowel written before its consonant sorts after
                # it, with a tertiary weight of its own.
                vowel = tab[c]
                out.append(tab[nxt])
                out.append((vowel[0], vowel[1], [PREVOWEL_TERTIARY] * len(vowel[2])))
                i += 2
                continue
            i += 1
            if c in tab:
                out.append(tab[c])
            elif 0xAC00 <= cp <= 0xD7A3:
                s = cp - 0xAC00
                jamo = [0x1100 + s // 588, 0x1161 + s % 588 // 28]
                if s % 28:
                    jamo.append(0x11A7 + s % 28)
                out.extend(tab[chr(j)] for j in jamo)
            elif ignorable(cp):
                continue
            elif 0xD800 <= cp <= 0xDFFF:
                return None
            else:
                out.append(([implicit(cp)], [C], [C]))
    return out


def key(text, tab, compressible, unsupported):
    """The engine's key of an NChar or NVarChar value: without its trailing
    spaces, unless it is all spaces."""
    text = text.rstrip(' ') or text
    es = elements(text, tab, unsupported)
    if es is None:
        return None
    ces, sec, ter = [], [], []
    for e in es:
        ces += e[0]
        sec += e[1]
        ter += e[2]
    return (compress_primary(ces, compressible) + b'\x01' + compress(sec, **SEC) + b'\x01'
            + compress(ter, **TER))


def main():
    cmd = sys.argv[1]
    if cmd == 'singles':
        write(sys.argv[2], [chr(c) for c in range(1, 0x10000) if not 0xD800 <= c < 0xE000])
    elif cmd == 'astral':
        write(sys.argv[2], [chr(c) for lo, hi in ASTRAL for c in range(lo, hi + 1)])
    elif cmd == 'contractions':
        # nkeys.py contractions <allkeys.txt> <out>: the contractions of the
        # Default Unicode Collation Element Table of UCA 5.1.0 (the version
        # ICU 4.0, the engine's, is built on), but the Thai and Lao vowels
        # that precede their consonants, which `elements` has a rule for.
        found = []
        for line in open(sys.argv[2]):
            if line.startswith(('#', '@')) or not line.strip():
                continue
            cps = [int(cp, 16) for cp in line.split(';')[0].split()]
            if len(cps) > 1 and cps[0] not in PREVOWEL_CONSONANTS:
                found.append(''.join(map(chr, cps)))
        write(sys.argv[3], found)
    elif cmd == 'pairs':
        singles = read_all(sys.argv[3:])
        write(sys.argv[2], [s + s for s in one_element(singles).values()])
    elif cmd == 'follow':
        singles = read_all(sys.argv[3:])
        write(sys.argv[2], [s + 'äB' for s in candidates(singles)])
    elif cmd == 'table':
        # nkeys.py table <pairs.keys> <follow.keys> <singles.keys>...
        tab, compressible, unsupported = build_table(read_all(sys.argv[4:]), read(sys.argv[2]),
                                                     read(sys.argv[3]))
        print('# The ICU root collation elements (as the original engine weighs NChar and')
        print('# NVarChar keys, `:en_US`) of the characters and contractions tflip builds')
        print('# such keys for, but those of characters weighed by formula (ideographs,')
        print('# Hangul syllables, unassigned characters). Worked out from the engine\'s')
        print('# keys by tools/ace/seek/nkeys.py, which describes the model. A line per')
        print('# string: its code points (joined by "+"), its primaries (each element\'s,')
        print('# joined by "."), its secondary and its tertiary weights, 05 for a')
        print('# compressible common one; "-" for none.')
        print('compressible ' + ' '.join('%02x' % b for b in sorted(compressible)))
        for text, (ces, sec, ter) in sorted(tab.items()):
            h = lambda ws: bytes(ws).hex() or '-'
            print('%s %s %s %s' % ('+'.join('%04x' % ord(c) for c in text),
                                   '.'.join(ce.hex() for ce in ces) or '-', h(sec), h(ter)))
        for text in sorted(unsupported):
            print('%s ? ? ?' % '+'.join('%04x' % ord(c) for c in text))
    elif cmd == 'random':
        tab, _, _ = load_table(sys.argv[2])
        rnd = random.Random(int(sys.argv[3]))
        singles = sorted(t for t in tab if len(t) == 1)
        contractions = sorted(t for t in tab if len(t) > 1)
        common = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 äöüßÄÖÜéèçñ.-,\''

        def pick():
            r = rnd.random()
            if r < 0.5:
                return rnd.choice(singles)
            if r < 0.6:
                return rnd.choice(common)
            if r < 0.65:
                return rnd.choice(contractions)
            if r < 0.7:
                return chr(rnd.randint(0x4E00, 0x9FFF))
            if r < 0.73:
                return chr(rnd.randint(0x3400, 0x4DBF))
            if r < 0.78:
                return chr(rnd.randint(0xAC00, 0xD7A3))
            if r < 0.82:
                return chr(rnd.randint(0x20000, 0x2A6DF))
            if r < 0.88:
                return chr(rnd.choice([rnd.randint(0xE000, 0xF8FF), rnd.randint(0x30000, 0x10FFFF),
                                       rnd.randint(0x2A700, 0x2F7FF), rnd.randint(0x1D800, 0x1FFFF)]))
            c = rnd.randint(0x100, 0xFFFF)
            return chr(c) if not 0xD800 <= c <= 0xDFFF else 'x'

        out = [''.join(pick() for _ in range(rnd.randint(1, 6))) for _ in range(int(sys.argv[4]))]
        write(sys.argv[5], out)
    elif cmd == 'mismatches':
        # nkeys.py mismatches <icu_root.txt> <field length> <keys>...: the
        # strings whose key is not the model's.
        tab, compressible, unsupported = load_table(sys.argv[2])
        length = int(sys.argv[3])
        for path in sys.argv[4:]:
            for line in open(path):
                t, k = line.split(maxsplit=1)
                text = bytes.fromhex(t).decode('utf-16-le')
                want = bytes.fromhex(k.strip())
                got = key(text.ljust(length), tab, compressible, unsupported)
                if got is not None and (got + bytes(len(want)))[:len(want)] != want:
                    print(t)
    elif cmd == 'check':
        tab, compressible, unsupported = load_table(sys.argv[2])
        length = int(sys.argv[4])
        bad = total = 0
        for line in open(sys.argv[3]):
            t, k = line.split(maxsplit=1)
            text = bytes.fromhex(t).decode('utf-16-le').ljust(length)
            want = bytes.fromhex(k.strip())
            got = key(text, tab, compressible, unsupported)
            if got is None:
                continue
            total += 1
            got = (got + bytes(len(want)))[:len(want)]
            if got != want:
                bad += 1
                if bad <= 10:
                    print(repr(text), got.hex(), want.hex())
        print('%d of %d differ' % (bad, total))
        sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
