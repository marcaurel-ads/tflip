#!/usr/bin/env python3
"""Searches for the table cipher's per-record (i0,j0) generator.

Input: the JSON that recover_ij.exe writes (a list of {"pw", "ij":[[i0,j0],...]}),
in which each record's starting RC4 counters (i0,j0) have been recovered against
the password's pristine permutation S0 = RC4_KSA(password[:20]). See FINDINGS.md
for what this generator is and why it matters.

Three analyses, all black-box (they only use the recovered (i0,j0) and the public
RC4 key schedule applied to the password):

  localize   -- per-S0-position marginal association (Cramér's V) with (i0,j0),
                across many passwords. A relevant position peaks above the noise
                floor sqrt((bins-1)/n). Flat => no single byte is marginally
                informative (does NOT by itself prove a wide seed: a mixing
                function of a few bytes is also flat -- see `search`).

  search     -- structured whole-construction search over three swept dimensions:
                  * index modulus -- positions = ((A*r+B)+step*k) mod M, with the
                    modulus M itself swept (all M<=256, so every window byte stays
                    in bounds -- the earlier search fixed M=256 and could run a
                    multi-byte window off the 256-entry array);
                  * index layout -- wrap (window may wrap the array) AND contig
                    (a genuine contiguous window kept in bounds, so the top of S0
                    is actually read);
                  * generator -- single-seed (LCG family), two-instance families
                    (multiply-with-carry, subtract-with-borrow, multiplicative-
                    combine, and one LCG combined with a record-seeded copy of
                    itself), AND genuine multi-component combined generators with
                    distinct constants (L'Ecuyer two-LCG, Wichmann-Hill three-LCG);
                  * seed packing -- byte order (LE/BE) AND bit-level packings
                    (nibble, xor-fold, sum);
                  * output map -- bit-slice (out>>s)&255 AND scaled floor(out*256/m)
                    (a real fractional output map for non-power-of-two moduli).
                A construction is accepted only if it predicts (i0,j0) EXACTLY on
                held-out passwords AND held-out records, including records past
                256. Reports the full space tried and any hit.

  selftest   -- positive control: plant a known construction from the search
                space into synthetic (i0,j0) over the same S0, and confirm
                `search` recovers it out-of-sample. Establishes that a negative
                `search` result means absence, not a powerless harness.

  selftest2  -- a HARDER positive control that exercises this round's new code
                paths together (genuine combined generator seeded from both inputs,
                scaled output map, contiguous multi-byte window at a non-256
                modulus). Validates the negative down to the fine construction
                details, not just the easy paths.

    ij_search.py localize  <recover_ij.json>
    ij_search.py search    <recover_ij.json>
    ij_search.py selftest  <recover_ij.json>
    ij_search.py selftest2 <recover_ij.json>
"""
import json
import sys
import itertools


def ksa(key):
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) & 255
        s[i], s[j] = s[j], s[i]
    return s


def pw_bytes(d):
    """Password bytes with the deliberate encoding, matching recover_ij.c:
    hex-decoded when present (any byte sequence), else the UTF-8 of "pw"."""
    if "pwhex" in d:
        return bytes.fromhex(d["pwhex"])
    return d["pw"].encode()


def load_np(path):
    import numpy as np
    D = json.load(open(path))
    raw = [pw_bytes(d) for d in D]
    # key = raw password bytes truncated to 20 (ADS's documented limit), same as C
    S0 = np.array([ksa(r[:20]) for r in raw], dtype=np.uint64)
    I0 = np.array([[x[0] for x in d["ij"]] for d in D], dtype=np.int64)
    J0 = np.array([[x[1] for x in d["ij"]] for d in D], dtype=np.int64)
    bad = int((I0 < 0).sum())
    if bad:
        # drop any password with an unrecovered record so the fit stays sound
        keep = (I0 >= 0).all(axis=1)
        print(f"WARNING: {bad} unrecovered (i0,j0) in {int((~keep).sum())} "
              f"password(s) — dropping those passwords")
        S0, I0, J0 = S0[keep], I0[keep], J0[keep]
        raw = [r for r, k in zip(raw, keep) if k]
    return raw, S0, I0.astype(np.uint64), J0.astype(np.uint64)


# --- localize: per-position marginal association --------------------------------

def localize(path):
    import numpy as np
    _, S0, I0, J0 = load_np(path)
    n, NR = I0.shape
    floor = (15 / n) ** 0.5
    print(f"passwords={n} records={NR} noise floor (16x16 bins) ~= {floor:.3f}")

    def cv(x, y, bins=16):
        bx = x * bins // 256
        by = y * bins // 256
        tab = np.zeros((bins, bins))
        np.add.at(tab, (bx, by), 1)
        tot = tab.sum()
        row = tab.sum(1, keepdims=True)
        col = tab.sum(0, keepdims=True)
        exp = row @ col / tot
        chi = np.nansum((tab - exp) ** 2 / np.where(exp > 0, exp, 1))
        return (chi / (tot * (bins - 1))) ** 0.5

    for name, OUT in (("i0", I0), ("j0", J0)):
        for r in range(min(NR, 8)):
            v = np.array([cv(S0[:, c], OUT[:, r]) for c in range(256)])
            pk = np.argsort(v)[-5:][::-1]
            flag = "  <-- peak clears floor" if v.max() > floor * 1.3 else ""
            print(f"{name} r={r}: floor={np.median(v):.3f} max={v.max():.3f} "
                  f"peaks={list(map(int, pk))}{flag}")


# --- search: modulus-swept, two-seed-inclusive, bit-packing whole-construction --

MODULI = [256, 255, 253, 251, 250, 240, 233, 199, 192, 149, 131, 128, 127, 64, 32]
AS = [1, 2, 3, 5, 7, 11, 16, 31]
BS = [0, 1, 7]
WIDTHS = [1, 2, 3, 4]
STEPS = [1, 2]
PACKINGS = ["LE", "BE", "nib", "xorfold", "summod"]
RMIX = ["none", "add", "xor"]
INDEXMODE = ["wrap", "contig"]
LCGS = {
    "msvc": (214013, 2531011, 1 << 32),
    "borland": (22695477, 1, 1 << 32),
    "minstd": (16807, 0, (1 << 31) - 1),
    "minstd2": (48271, 0, (1 << 31) - 1),
    "ansic": (1103515245, 12345, 1 << 31),
}


def _positions(M, A, B, w, step, imode, r):
    """w indices into S0[0..255], always in bounds.
    wrap   -- base then +step*k each reduced mod M (window may wrap the array).
    contig -- base reduced so the whole window base..base+step*(w-1) fits in
              [0,255] with NO per-byte re-reduction, so a genuine contiguous
              window into the TOP of S0 (positions 253-255) is actually tested."""
    span = step * (w - 1)
    if imode == "wrap":
        base = (A * r + B) % M
        return [(base + step * k) % M for k in range(w)]
    hi = min(M, 256 - span)            # base range that keeps the window in bounds
    base = (A * r + B) % hi if hi > 0 else 0
    return [base + step * k for k in range(w)]


def _pack(cols, how):
    import numpy as np
    w = len(cols)
    acc = np.zeros_like(cols[0])
    if how == "LE":
        for k, b in enumerate(cols):
            acc = acc | (b << np.uint64(8 * k))
    elif how == "BE":
        for k, b in enumerate(cols):
            acc = acc | (b << np.uint64(8 * (w - 1 - k)))
    elif how == "nib":
        for k, b in enumerate(cols):
            acc = acc | ((b & np.uint64(0xF)) << np.uint64(4 * k))
    elif how == "xorfold":
        for b in cols:
            acc = acc ^ b
    elif how == "summod":
        for b in cols:
            acc = acc + b
    else:
        raise ValueError(how)
    return acc


def _generators():
    """name -> (fn, m_eff). fn(sd, rec) returns two per-record outputs in [0,m_eff).
    m_eff is the output's modulus, used by the SCALED extraction floor(out*256/m_eff)
    -- which is a real output map, not a bit-slice, when m_eff is not a power of 2."""
    import numpy as np
    MASK32 = np.uint64(0xFFFFFFFF)
    M32 = 1 << 32
    G = {}

    def mk_lcg(a, c, m):
        a, c, m = np.uint64(a), np.uint64(c), np.uint64(m)

        def f(sd, rec):
            s1 = (a * (sd % m) + c) % m
            return s1, (a * s1 + c) % m
        return f

    def mk_two(a, c, m, op):
        # two instances of ONE LCG: one seeded by the S0 window, one by the record.
        a, c, m = np.uint64(a), np.uint64(c), np.uint64(m)

        def f(sd, rec):
            rr = np.uint64(rec)
            s1 = (a * (sd % m) + c) % m
            s2 = (a * rr + c) % m
            s1b = (a * s1 + c) % m
            s2b = (a * s2 + c) % m
            if op == "sum":
                return (s1 + s2) % m, (s1b + s2b) % m
            if op == "xor":
                return (s1 ^ s2), (s1b ^ s2b)
            return (s1 - s2) % m, (s1b - s2b) % m
        return f

    def mk_combined(a1, m1, a2, m2):
        # L'Ecuyer-style combined generator: TWO LCGs with DISTINCT constants,
        # output z = (x1 - x2) mod (m1-1), a fraction in [0,m1-1). Window seeds the
        # first component, the record seeds the second.
        a1, m1, a2, m2 = np.uint64(a1), np.uint64(m1), np.uint64(a2), np.uint64(m2)
        mm = m1 - np.uint64(1)

        def f(sd, rec):
            rr = np.uint64(rec)
            x1 = (a1 * (sd % m1)) % m1
            x1 = np.where(x1 == 0, np.uint64(1), x1)
            x2 = (a2 * (rr % m2)) % m2
            x2 = np.where(x2 == 0, np.uint64(1), x2)
            z0 = (x1 + mm - x2) % mm
            x1b = (a1 * x1) % m1
            x2b = (a2 * x2) % m2
            z1 = (x1b + mm - x2b) % mm
            return z0, z1
        return f, int(mm)

    def mk_wichmann():
        # Wichmann-Hill: three LCGs mod distinct primes; output is a fraction.
        m1, m2, m3 = 30269, 30307, 30323
        a1, a2, a3 = 171, 172, 170
        D = m1 * m2 * m3
        c1, c2, c3 = m2 * m3, m1 * m3, m1 * m2
        m1u, m2u, m3u = np.uint64(m1), np.uint64(m2), np.uint64(m3)
        a1u, a2u, a3u = np.uint64(a1), np.uint64(a2), np.uint64(a3)
        c1u, c2u, c3u, Du = map(np.uint64, (c1, c2, c3, D))

        def f(sd, rec):
            rr = np.uint64(rec)
            s1 = (a1u * (sd % m1u)) % m1u
            s2 = (a2u * (rr % m2u)) % m2u
            s3 = (a3u * ((sd ^ rr) % m3u)) % m3u
            w0 = (s1 * c1u + s2 * c2u + s3 * c3u) % Du
            s1b = (a1u * s1) % m1u
            s2b = (a2u * s2) % m2u
            s3b = (a3u * s3) % m3u
            w1 = (s1b * c1u + s2b * c2u + s3b * c3u) % Du
            return w0, w1
        return f, D

    def mk_mwc(a):
        A = np.uint64(a)

        def f(sd, rec):
            x = sd & MASK32
            t = A * x + (np.uint64(rec) & MASK32)
            x1 = t & MASK32
            t2 = A * x1 + (t >> np.uint64(32))
            return x1, t2 & MASK32
        return f

    def mk_swb():
        def f(sd, rec):
            a = sd & MASK32
            b = (np.uint64(rec) * np.uint64(2654435761)) & MASK32
            return (a - b) & MASK32, (b - a) & MASK32
        return f

    def mk_mul(K1, K2, C):
        k1, k2, c = np.uint64(K1), np.uint64(K2), np.uint64(C)

        def f(sd, rec):
            rr = np.uint64(rec)
            o0 = (sd * k1 + rr * k2 + c) & MASK32
            return o0, (o0 * k1 + rr * k2 + c) & MASK32
        return f

    for fam, (a, c, m) in LCGS.items():
        G[f"lcg:{fam}"] = (mk_lcg(a, c, m), m)
        for op in ("sum", "xor", "sub"):
            G[f"two:{fam}/{op}"] = (mk_two(a, c, m, op), m)
    # genuine combined generators (distinct constants per component)
    G["comb:lecuyer"] = mk_combined(40014, 2147483563, 40692, 2147483399)
    G["comb:wichmann"] = mk_wichmann()
    for a in (698769069, 4294957665, 18000, 30903):
        G[f"mwc:{a}"] = (mk_mwc(a), M32)
    G["swb"] = (mk_swb(), M32)
    for (K1, K2, C) in ((2654435761, 40503, 0),
                        (2246822519, 3266489917, 374761393),
                        (214013, 2531011, 2531011)):
        G[f"mul:{K1}/{K2}"] = (mk_mul(K1, K2, C), M32)
    return G


# shift == SCALE in an extraction mode means floor(out*256/m_eff), not a bit-slice.
SCALE = -1


def _extract_modes():
    modes = []
    for si in (0, 8, 16):
        modes.append(("a", si, "b", si))
    for si in (0, 8):
        for sj in (8, 16):
            modes.append(("a", si, "a", sj))
    modes.append(("a", SCALE, "b", SCALE))  # scaled output map (finding 2)
    return modes


def _emit(out0, out1, m_eff, mode):
    import numpy as np
    srci, shi, srcj, shj = mode
    oi = out0 if srci == "a" else out1
    oj = out0 if srcj == "a" else out1
    me = np.uint64(m_eff)

    def take(o, sh):
        if sh == SCALE:
            return (o * np.uint64(256)) // me
        return (o >> np.uint64(sh)) & np.uint64(255)
    return take(oi, shi), take(oj, shj)


def _search_core(S0, I0, J0, verbose=True):
    import numpy as np
    n, NR = I0.shape
    half = n // 2
    FIT = np.arange(half)
    HO = np.arange(half, n)
    PRUNE = np.arange(min(40, half))
    FIT_R = [r for r in (1, 2, 257, 258) if r < NR]
    HO_R = [r for r in (3, 100, 200, 259, NR - 1) if r < NR]
    PRUNE_R = [r for r in (1, 257) if r < NR] or [min(1, NR - 1)]
    GEN = _generators()
    EXT = _extract_modes()
    if verbose:
        print(f"sample: passwords={n} records/pw={NR} (max record index {NR-1})")
        print(f"fit pws={len(FIT)} holdout pws={len(HO)} "
              f"fit recs={FIT_R} holdout recs={HO_R}")

    def seed_for(M, A, B, w, step, imode, how, rmix, rec, rows):
        cols = [S0[rows][:, p] for p in _positions(M, A, B, w, step, imode, rec)]
        sd = _pack(cols, how)
        if rmix == "add":
            sd = sd + np.uint64(rec)
        elif rmix == "xor":
            sd = sd ^ np.uint64(rec)
        return sd

    def full_check(predfn):
        for r in FIT_R:
            pi, pj = predfn(r, FIT)
            if not (np.array_equal(pi, I0[FIT, r]) and np.array_equal(pj, J0[FIT, r])):
                return None
        for r in HO_R:
            pi, pj = predfn(r, HO)
            if not (np.array_equal(pi, I0[HO, r]) and np.array_equal(pj, J0[HO, r])):
                return False
        return True

    tgtI = {r: I0[PRUNE, r] for r in PRUNE_R}
    tgtJ = {r: J0[PRUNE, r] for r in PRUNE_R}
    tries = 0
    seedbuilds = 0
    hits = []
    for (M, A, B, w, step, imode, how, rmix) in itertools.product(
            MODULI, AS, BS, WIDTHS, STEPS, INDEXMODE, PACKINGS, RMIX):
        if w == 1 and (step == 2 or how in ("BE", "nib", "xorfold", "summod")):
            continue  # width-1: these packings collapse to the single byte
        if w == 1 and imode == "contig":
            continue  # width-1: contig and wrap read the same single byte
        sds = {r: seed_for(M, A, B, w, step, imode, how, rmix, r, PRUNE)
               for r in PRUNE_R}
        seedbuilds += 1
        for gname, (gfn, meff) in GEN.items():
            outs = {r: gfn(sds[r], r) for r in PRUNE_R}
            for mode in EXT:
                tries += 1
                if any(not (np.array_equal(_emit(*outs[r], meff, mode)[0], tgtI[r])
                            and np.array_equal(_emit(*outs[r], meff, mode)[1], tgtJ[r]))
                       for r in PRUNE_R):
                    continue

                def predfn(r, rows, M=M, A=A, B=B, w=w, step=step, imode=imode,
                           how=how, rmix=rmix, gfn=gfn, meff=meff, mode=mode):
                    o0, o1 = gfn(seed_for(M, A, B, w, step, imode, how, rmix, r, rows), r)
                    return _emit(o0, o1, meff, mode)
                res = full_check(predfn)
                if res is None:
                    continue
                label = (f"{gname} idx({A}r+{B})%{M}/{imode} w{w}s{step} {how} "
                         f"rmix={rmix} ext={mode}")
                hits.append((label, res))
                if verbose:
                    print(f"  CANDIDATE {label}  held-out={res}")
    confirmed = [h for h in hits if h[1] is True]
    if verbose:
        print(f"\nindex-configs built: {seedbuilds}")
        print(f"generator x extraction combinations tried: {tries}")
        print(f"generators: {len(GEN)}  extraction modes: {len(EXT)} "
              f"(incl. scaled output map)")
        print(f"candidates passing prune+fit: {len(hits)}  "
              f"held-out confirmed: {len(confirmed)}")
        if not confirmed:
            print("\nNO hit: search exhausted over bit-slice AND scaled outputs, "
                  "single- and multi-component generators, wrap AND contiguous "
                  "indexing, modulus-swept, bit-packing (see FINDINGS.md).")
    return confirmed


def search(path):
    _, S0, I0, J0 = load_np(path)
    _search_core(S0, I0, J0)


def _plant(S0, M, A, B, w, step, imode, how, rmix, gname, mode, NR=300):
    """Generate synthetic (i0,j0) over the real S0 from ONE in-space construction,
    using the search's own building blocks so it is exactly representable."""
    import numpy as np
    n = S0.shape[0]
    gfn, meff = _generators()[gname]
    I = np.zeros((n, NR), dtype=np.uint64)
    J = np.zeros((n, NR), dtype=np.uint64)
    for r in range(NR):
        cols = [S0[:, p] for p in _positions(M, A, B, w, step, imode, r)]
        sd = _pack(cols, how)
        if rmix == "add":
            sd = sd + np.uint64(r)
        elif rmix == "xor":
            sd = sd ^ np.uint64(r)
        pi, pj = _emit(*gfn(sd, r), meff, mode)
        I[:, r], J[:, r] = pi, pj
    return I, J


def selftest(path):
    """Simple positive control: plant a known in-space construction (single LCG,
    bit-slice output, width-1 index) and confirm `search` recovers it."""
    _, S0, _, _ = load_np(path)
    I, J = _plant(S0, 251, 2, 1, 1, 1, "wrap", "LE", "none",
                  "lcg:msvc", ("a", 0, "b", 0))
    print("planted: lcg:msvc idx(2r+1)%251/wrap w1 ext=('a',0,'b',0)")
    confirmed = _search_core(S0, I, J)
    ok = any("lcg:msvc" in lab and "%251" in lab for lab, _ in confirmed)
    print("SELFTEST", "PASS" if ok else "FAIL", "--",
          "search recovers the planted construction out-of-sample" if ok
          else "search FAILED to recover a known in-space generator")


def selftest2(path):
    """Hard positive control that exercises THIS round's new code paths together:
    a genuine multi-component combined generator (L'Ecuyer two-LCG) whose two
    components are seeded from the two distinct inputs (the S0-window value and the
    record number), a SCALED output map floor(out*256/m) rather than a bit-slice,
    and a CONTIGUOUS multi-byte window at a non-256 modulus index. If `search`
    fails to recover this, the new paths have a bug and a 0-hits result is an
    artifact."""
    M, A, B, w, step, imode, how = 251, 2, 1, 2, 1, "contig", "LE"
    gname, mode = "comb:lecuyer", ("a", SCALE, "b", SCALE)
    _, S0, _, _ = load_np(path)
    I, J = _plant(S0, M, A, B, w, step, imode, how, "none", gname, mode)
    print(f"planted: {gname} idx({A}r+{B})%{M}/{imode} w{w}s{step} {how} "
          f"ext=scaled  (combined gen seeded from window+record, scaled output, "
          f"contiguous {w}-byte window at mod {M})")
    confirmed = _search_core(S0, I, J)
    ok = any(gname in lab and "/contig" in lab and f"%{M}" in lab and "-1" in lab
             for lab, _ in confirmed)
    print("SELFTEST2", "PASS" if ok else "FAIL", "--",
          "search recovers the combined/scaled/contiguous plant out-of-sample" if ok
          else "search FAILED to recover a known new-path construction (BUG)")


def main():
    cmds = {"localize": localize, "search": search,
            "selftest": selftest, "selftest2": selftest2}
    if len(sys.argv) < 3 or sys.argv[1] not in cmds:
        print(__doc__)
        return
    cmds[sys.argv[1]](sys.argv[2])


if __name__ == "__main__":
    main()
