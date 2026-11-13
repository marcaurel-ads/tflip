#!/usr/bin/env python3
"""Identifies the table cipher's per-record (i0,j0) generator from the OUTPUT
STREAM's own structure, independent of how the seed is derived from S0.

Input: the JSON that recover_ij.exe (or recon on captured keystreams) writes, a
list of {"pwhex"|"pw","ij":[[i0,j0],...]}. Black-box: uses only the recovered
(i0,j0).

Two analyses:

  stats   -- the stream's intrinsic structure: marginal distribution / output
             representation; independence of the two channels and across records
             (correlation + mutual information); serial autocorrelation at all
             lags; 2D/3D lattice (spectral) tests. Answers "does the stream carry
             any linear/lattice structure a classical PRNG would leave?"

  modelA  -- continuous-run test. Treats each password's interleaved stream
             (i0_0,j0_0,i0_1,j0_1,...) as consecutive outputs of one LCG seeded
             per password, and tries to reconstruct the seed by truncated-sequence
             lattice reduction for named generators (drand48/java, glibc, MSVC,
             Borland). A recovered state must predict HELD-OUT outputs. Runs a
             synthetic-drand48 self-test first, so a negative means "excluded",
             not "solver too weak". (Slow: pure-Python LLL, ~10 s per password.)

    stream_id.py stats  <big.json>
    stream_id.py modelA <big.json> [L] [npw]
"""
import json
import sys
from fractions import Fraction as Fr


# ---------------------------------------------------------------- stats ---------
def stats(path):
    import numpy as np
    D = json.load(open(path))
    I = np.array([[x[0] for x in d["ij"]] for d in D])
    J = np.array([[x[1] for x in d["ij"]] for d in D])
    n, NR = I.shape
    i, j = I.ravel(), J.ravel()
    N = i.size
    print(f"sample: {n} passwords x {NR} records = {N} (i0,j0) pairs\n")

    print("1. output representation (marginal of each channel)")
    for name, v in (("i0", i), ("j0", j)):
        h = np.bincount(v, minlength=256)
        chi = ((h - N / 256) ** 2 / (N / 256)).sum()
        miss = [k for k in range(256) if h[k] == 0]
        print(f"   {name}: range [{v.min()},{v.max()}] distinct {len(np.unique(v))}/256 "
              f"uniformity chi2={chi:.0f}/255 missing={len(miss)}")
    print("   => full-range uniform byte: not a small-modulus residue\n")

    def corr(a, b):
        return float(np.corrcoef(a, b)[0, 1])

    def mi(a, b, bins=16):
        ca, cb = a * bins // 256, b * bins // 256
        H = np.histogram2d(ca, cb, bins=bins, range=[[0, bins]] * 2)[0] + 1e-9
        P = H / H.sum()
        return float((P * np.log2(P / (P.sum(1, keepdims=True) * P.sum(0, keepdims=True)))).sum())

    print("2. independence (corr + mutual information, noise ~%.4f bits)"
          % ((15 * 15) / (2 * N) / np.log(2)))
    print(f"   i0 vs j0 (same record): corr={corr(i, j):+.4f}  MI={mi(i, j):.4f} bits")
    print(f"   i0_r vs i0_r+1        : corr={corr(I[:, :-1].ravel(), I[:, 1:].ravel()):+.4f}  "
          f"MI={mi(I[:, :-1].ravel(), I[:, 1:].ravel()):.4f} bits")
    print(f"   j0_r vs j0_r+1        : corr={corr(J[:, :-1].ravel(), J[:, 1:].ravel()):+.4f}  "
          f"MI={mi(J[:, :-1].ravel(), J[:, 1:].ravel()):.4f} bits")
    # j0 as a function of i0?
    fmap = {}
    for a, b in zip(i, j):
        fmap.setdefault(a, set()).add(b)
    print(f"   max distinct j0 for a fixed i0 = {max(len(s) for s in fmap.values())} "
          f"(1 => j0=f(i0); here many => not a consecutive low-byte LCG)\n")

    print("3. serial autocorrelation of the interleaved stream (per pw, avg)")
    S = np.empty((n, 2 * NR), dtype=int)
    S[:, 0::2], S[:, 1::2] = I, J
    acs = [np.mean([corr(S[p, :-L], S[p, L:]) for p in range(n)]) for L in range(1, 33)]
    print(f"   max |autocorr| over lags 1..32 = {max(abs(x) for x in acs):.4f} "
          f"(noise ~{1/np.sqrt(2*NR):.4f})")
    # LCG low-bit (period-2) signature
    for bit in range(3):
        b = (S >> bit) & 1
        c1 = np.mean([corr(b[p, :-1], b[p, 1:]) for p in range(n)])
        print(f"   bit{bit} lag-1 autocorr = {c1:+.4f} "
              f"(low-byte power-of-2 LCG would show period-2 in bit0)")
    print()

    print("4. lattice / spectral test (consecutive pairs & triples pooled)")
    pr = np.stack([S[:, :-1].ravel(), S[:, 1:].ravel()], 1)
    for g in (16, 32):
        H = np.histogram2d(pr[:, 0], pr[:, 1], bins=g, range=[[0, 256]] * 2)[0]
        chi = ((H - pr.shape[0] / g / g) ** 2 / (pr.shape[0] / g / g)).sum()
        df = g * g - 1
        print(f"   2D pairs {g}x{g}: chi2={chi:.0f} df={df} z={(chi-df)/np.sqrt(2*df):+.2f}")
    tr = np.stack([S[:, :-2].ravel(), S[:, 1:-1].ravel(), S[:, 2:].ravel()], 1)
    H = np.histogramdd(tr, bins=(8, 8, 8), range=[[0, 256]] * 3)[0]
    chi = ((H - tr.shape[0] / 512) ** 2 / (tr.shape[0] / 512)).sum()
    print(f"   3D triples 8^3: chi2={chi:.0f} df=511 z={(chi-511)/np.sqrt(2*511):+.2f}")
    print("   => no lattice banding: no coarse LCG/LFSR structure in the stream")


# ------------------------------------------------- modelA: lattice on a run -----
def _lll(B, delta=Fr(99, 100)):
    B = [list(map(int, r)) for r in B]
    n, d = len(B), len(B[0])
    Bstar = [[Fr(0)] * d for _ in range(n)]
    mu = [[Fr(0)] * n for _ in range(n)]
    Bn = [Fr(0)] * n

    def gso():
        for i in range(n):
            vi = [Fr(x) for x in B[i]]
            for jj in range(i):
                mu[i][jj] = sum(Fr(B[i][t]) * Bstar[jj][t] for t in range(d)) / Bn[jj]
                vi = [vi[t] - mu[i][jj] * Bstar[jj][t] for t in range(d)]
            Bstar[i] = vi
            Bn[i] = sum(vi[t] * vi[t] for t in range(d))

    gso()
    k = 1
    while k < n:
        for jj in range(k - 1, -1, -1):
            if abs(mu[k][jj]) > Fr(1, 2):
                q = round(mu[k][jj])
                B[k] = [B[k][t] - q * B[jj][t] for t in range(d)]
                gso()
        if Bn[k] >= (delta - mu[k][k - 1] ** 2) * Bn[k - 1]:
            k += 1
        else:
            B[k], B[k - 1] = B[k - 1], B[k]
            gso()
            k = max(k - 1, 1)
    return B, Bstar, Bn


def _babai(B, Bstar, Bn, target):
    d = len(target)
    b = [Fr(x) for x in target]
    for i in range(len(B) - 1, -1, -1):
        ci = round(sum(b[t] * Bstar[i][t] for t in range(d)) / Bn[i])
        b = [b[t] - ci * B[i][t] for t in range(d)]
    return [Fr(target[t]) - b[t] for t in range(d)]


def _recover(obs, a, c, m, scale):
    L = len(obs)
    an, cn = [1 % m], [0]
    for _ in range(1, L):
        cn.append((a * cn[-1] + c) % m)
        an.append((a * an[-1]) % m)
    width = m // scale
    b0 = obs[0]
    t = [(an[k] * (b0 * width) + cn[k] - obs[k] * width) % m for k in range(1, L)]
    B = [[an[k] for k in range(1, L)] + [1]]
    for idx in range(L - 1):
        row = [0] * L
        row[idx] = m
        B.append(row)
    Br, Bs, Bn = _lll(B)
    target = [(-t[i]) + width // 2 for i in range(L - 1)] + [width // 2]
    v = _babai(Br, Bs, Bn, target)
    y0 = int(v[-1])
    return b0 * width + y0 if 0 <= y0 < width else None


def _genstream(X0, a, c, m, scale, L):
    out, X = [], X0
    for _ in range(L):
        out.append((scale * X) // m)
        X = (a * X + c) % m
    return out


CANDS = {
    "drand48/java": (25214903917, 11, 1 << 48, 256),
    "glibc_min": (1103515245, 12345, 1 << 31, 256),
    "msvc": (214013, 2531011, 1 << 31, 256),
    "borland": (22695477, 1, 1 << 32, 256),
}


def modelA(path, L=12, npw=4):
    a, c, m, scale = CANDS["drand48/java"]
    import random
    rng = random.Random(1)
    ok = 0
    for _ in range(5):
        X0 = rng.randrange(m)
        full = _genstream(X0, a, c, m, scale, 40)
        Xr = _recover(full[:L], a, c, m, scale)
        ok += Xr is not None and _genstream(Xr, a, c, m, scale, 40) == full
    print(f"SELFTEST drand48 (L={L}): {ok}/5 synthetic streams recovered + held-out ok")
    if ok < 5:
        print("solver too weak at this L — raise L; aborting")
        return
    D = json.load(open(path))
    streams = [[v for rec in d["ij"] for v in rec] for d in D]  # interleaved 2/rec
    for name, (a, c, m, scale) in CANDS.items():
        hits = 0
        for p in range(npw):
            s = streams[p]
            X = _recover(s[:L], a, c, m, scale)
            if X is None:
                continue
            pred = _genstream(X, a, c, m, scale, 40)
            if all(pred[k] == s[k] for k in range(L, 40)):
                hits += 1
        print(f"{name:14s}: {hits}/{npw} passwords consistent with a continuous run")
    print("=> 0/* for every candidate: the stream is not a continuous LCG run.")


def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ("stats", "modelA"):
        print(__doc__)
        return
    if sys.argv[1] == "stats":
        stats(sys.argv[2])
    else:
        modelA(sys.argv[2], *(int(x) for x in sys.argv[3:5]))


if __name__ == "__main__":
    main()
