#!/usr/bin/env python3
"""Summarises the encryption probes (crypto.json, dict.json) from run.sh.

For table encryption it recovers the keystream and reports:
  - whether the API hands out ciphertext when a table is opened without the
    password (the question of reading encrypted data through the API);
  - whether the keystream resets per record or advances (an IV / record number);
  - how the keystream depends on the password (length, case, which bytes).

For the dictionary it reports whether the same plaintext encrypts to the same
bytes in two different .add files -- the fixed-keystream (breakable) versus
salted (binary-resident secret) question.

Nothing here is disassembly: it reasons only about the API's inputs and outputs
and the bytes the engine wrote to its own files.
"""
import json
import sys


def unhex(s):
    return bytes.fromhex(s) if s else b""


def xor(a, b):
    n = min(len(a), len(b))
    return bytes(a[i] ^ b[i] for i in range(n))


def runs_of_zero(ks):
    """Length of the all-zero prefix: keystream bytes we could not see (if the
    plaintext there was also zero and the cipher left it unchanged)."""
    n = 0
    for b in ks:
        if b:
            break
        n += 1
    return n


def show_bytes(b, limit=48):
    h = b[:limit].hex()
    return h + ("..." if len(b) > limit else "")


def rc4_keystream(key, n):
    """Classic RC4 keystream of n bytes for the given key bytes."""
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) & 0xFF
        s[i], s[j] = s[j], s[i]
    out = bytearray()
    i = j = 0
    for _ in range(n):
        i = (i + 1) & 0xFF
        j = (j + s[i]) & 0xFF
        s[i], s[j] = s[j], s[i]
        out.append(s[(s[i] + s[j]) & 0xFF])
    return bytes(out)


def try_rc4_derivations(pw, keystream, recno):
    """Test plausible password->RC4-key derivations against the recovered
    keystream (the data part, after the record prefix). Returns the matching
    derivation's name, or None."""
    pwb = pw.encode("latin-1", "replace")
    candidates = {
        "password bytes": pwb,
        "password + recno(1,le32)": pwb + recno.to_bytes(4, "little"),
        "password + recno(1,be32)": pwb + recno.to_bytes(4, "big"),
        "recno(1,le32) + password": recno.to_bytes(4, "little") + pwb,
        "upper(password)": pw.upper().encode("latin-1", "replace"),
    }
    for name, key in candidates.items():
        if not key:
            continue
        ks = rc4_keystream(key, len(keystream))
        if ks[: len(keystream)] == keystream:
            return name
        # keystream may start part-way in (prefix bytes consumed first)
        for skip in (0, 1, 5):
            if rc4_keystream(key, len(keystream) + skip)[skip:] == keystream:
                return f"{name} (skip {skip})"
    return None


def analyze_crypto(path):
    print("=== table encryption (crypto.json) ===")
    results = json.load(open(path))
    keystreams = {}
    for r in results:
        pw = r.get("password", "")
        print(f"\npassword {pw!r}:")
        print(f"  create rc={r.get('create_rc')} enable rc={r.get('enable_rc')} "
              f"encrypt rc={r.get('encrypt_rc')} appends="
              f"{[r.get(f'append{i}_rc') for i in range(4)]}")
        print(f"  record_length={r.get('record_length')} count={r.get('record_count')}")

        # Does the API give ciphertext without the password?
        nopw0 = unhex(r.get("nopw_rec0", ""))
        pw0 = unhex(r.get("pw_rec0", ""))
        if r.get("open_nopw_rc"):
            print(f"  open without password: rc={r.get('open_nopw_rc')} (refused)")
        else:
            print(f"  open without password: ok; is_table_encrypted="
                  f"{r.get('is_table_encrypted')} is_record_encrypted="
                  f"{r.get('is_record_encrypted')}")
            if nopw0 and pw0:
                if nopw0 == pw0:
                    print("  API returned the SAME bytes with and without the "
                          "password -> no ciphertext via the API (use the file)")
                else:
                    print("  API returned DIFFERENT bytes without the password "
                          "-> that is the ciphertext, readable through the API")

        # Keystream from the all-zero record 0: ciphertext == keystream.
        # Prefer the file; the record images may include a status byte, so we
        # align by searching the file for the ciphertext of record 0 is
        # overkill -- instead use the API ciphertext if it differs from plain,
        # else fall back to pw/nopw XOR.
        ct0 = nopw0 if (nopw0 and nopw0 != pw0) else None
        ct1 = unhex(r.get("nopw_rec1", "")) or None
        if ct0 is None and pw0 and nopw0:
            ct0 = nopw0
        if ct0:
            # record 0 and 1 are both all-zero plaintext: ciphertext == keystream
            ks0 = ct0
            keystreams[pw] = ks0
            print(f"  keystream (record 0) = {show_bytes(ks0)}")
            # The leading constant bytes are the unencrypted record prefix;
            # the rest is keystream over the (all-zero) data. Test RC4.
            prefix = 5
            data_ks = ks0[prefix:]
            hit = None
            for recno in (0, 1):
                hit = try_rc4_derivations(pw, data_ks, recno)
                if hit:
                    print(f"  RC4 MATCH: key = {hit}, recno {recno}")
                    break
            if not hit:
                print("  (no simple RC4 derivation matched; see JSON for raw bytes)")
            if ct1:
                same = ct1[:len(ks0)] == ks0[:len(ct1)]
                print(f"  record 1 keystream {'MATCHES' if same else 'DIFFERS from'} "
                      f"record 0 -> keystream {'resets per record' if same else 'advances (IV/recno)'}")
        else:
            print("  could not isolate the keystream from the API; inspect "
                  "'file' in the JSON (header + encrypted records)")

    # How the keystream depends on the password.
    if keystreams:
        print("\n-- keystream vs password --")
        items = list(keystreams.items())
        base_pw, base_ks = items[0]
        for pw, ks in items:
            common = sum(1 for a, b in zip(ks, base_ks) if a == b)
            total = min(len(ks), len(base_ks))
            print(f"  {pw!r:20} agrees with {base_pw!r} in {common}/{total} bytes"
                  + (" (identical)" if ks == base_ks else ""))


def common_prefix_len(images):
    """Longest leading run of bytes that is identical across every record image.
    These are the plaintext record prefix/metadata the engine leaves in the
    clear; the keystream over the all-zero data follows."""
    if not images:
        return 0
    n = min(len(x) for x in images)
    for i in range(n):
        b = images[0][i]
        if any(x[i] != b for x in images):
            return i
    return n


def keystreams_of(run):
    """(prefix_len, [keystream per record]) for one stream.c run: each all-zero
    record's ciphertext past the constant prefix is that record's keystream."""
    images = [unhex(h) for h in run.get("records", [])]
    p = common_prefix_len(images)
    return p, [img[p:] for img in images]


def smallest_period(kss):
    """Smallest p>0 with ks[r] == ks[r+p] for every record r, over the overlap
    length; None if no period up to len-1."""
    n = len(kss)
    for p in range(1, n):
        if all(kss[r][: len(kss[r + p])] == kss[r + p][: len(kss[r])]
               for r in range(n - p)):
            return p
    return None


def analyze_stream(path):
    print("\n=== table item structure (stream.json) ===")
    runs = json.load(open(path))
    by_pw = {}
    for r in runs:
        pw = r.get("password", "")
        p, kss = keystreams_of(r)
        kept = [k for k in kss if k]
        print(f"\n{r.get('file')}: pw={pw!r} fields={r.get('fields')} "
              f"data_bytes={r.get('data_bytes')} records={r.get('record_count')} "
              f"open_nopw_rc={r.get('open_nopw_rc')}")
        if not kept:
            print("  no record ciphertext captured (see open_nopw_rc)")
            continue
        print(f"  constant record prefix = {p} bytes "
              f"({show_bytes(unhex(r['records'][0])[:p]) if p else 'none'})")
        if len(kept) >= 2:
            print(f"  record 0 vs record 1 keystream: "
                  f"{'SAME (resets per record)' if kept[0] == kept[1] else 'DIFFERENT (varies per record)'}")
        per = smallest_period(kept)
        print(f"  period in record number: "
              f"{per if per is not None else f'none up to {len(kept) - 1}'}")
        by_pw.setdefault(pw, []).append((r.get("data_bytes"), kss))

    # Same password, different record width: is the keystream selected by the
    # record NUMBER (width-independent) or by a byte offset into one stream?
    for pw, runs_pw in by_pw.items():
        widths = {w: kss for w, kss in runs_pw}
        if len(widths) < 2:
            continue
        ws = sorted(widths)
        a_w, b_w = ws[0], ws[-1]
        ka, kb = widths[a_w], widths[b_w]
        n = min(len(ka), len(kb))
        same = 0
        for r in range(n):
            m = min(len(ka[r]), len(kb[r]))
            if m and ka[r][:m] == kb[r][:m]:
                same += 1
        print(f"\n-- width test, pw={pw!r}: {a_w}- vs {b_w}-byte records --")
        if same == n:
            print(f"  record keystreams match across widths for all {n} records "
                  "-> keystream is selected by the RECORD NUMBER, independent of "
                  "record byte length (re-keyed per item, not one continuous stream)")
        elif same == 0:
            print(f"  record keystreams differ across widths for all {n} records "
                  "-> position depends on cumulative record BYTES (one continuous "
                  "stream), or on width as well as number")
        else:
            print(f"  record keystreams match across widths for {same}/{n} records "
                  "-> mixed; inspect the per-record bytes in stream.json")


def analyze_dict(path):
    print("\n=== dictionary properties (dict.json) ===")
    results = {r["file"]: r for r in json.load(open(path))}
    for f, r in results.items():
        print(f"\n{f}: ddcreate rc={r.get('ddcreate_rc')} set_admin rc="
              f"{r.get('set_admin_pw_rc')} create_user rc={r.get('create_user_rc')} "
              f"len={r.get('file_length')}")

    a = results.get("da.add")
    b = results.get("db.add")
    c = results.get("dc.add")
    d = results.get("dd.add")
    e = results.get("de.add")
    f = results.get("df.add")
    if a and (e or f):
        ta = a.get("create_time")
        te = (e or f).get("create_time")
        if ta is not None and te is not None:
            print(f"\ncreate_time: da={ta}, "
                  f"d{'e' if e else 'f'}={te} (+{te - ta}s)")
    if a and b:
        fa, fb = unhex(a["file_hex"]), unhex(b["file_hex"])
        if fa == fb:
            print("\nda.add and db.add (same passwords, created in the same "
                  "second) are BYTE-IDENTICAL -> the encryption is deterministic; "
                  "no random per-dictionary salt. Any per-dictionary variation is "
                  "the creation timestamp (see the delay test), which is "
                  "low-entropy and stored in the file, not a random nonce.")
        else:
            diff = [i for i in range(min(len(fa), len(fb))) if fa[i] != fb[i]]
            print(f"\nda.add and db.add (same passwords) DIFFER in {len(diff)} "
                  f"bytes (+{abs(len(fa)-len(fb))} length), offsets "
                  f"{diff[0]}..{diff[-1]} -> a per-dictionary IV. Re-run with the "
                  "two created in the same second to tell a timestamp IV (knowable) "
                  "from a random salt (unknowable).")
    for other, label in ((c, "admin password"), (d, "user password")):
        if a and other:
            fa, fo = unhex(a["file_hex"]), unhex(other["file_hex"])
            diff = [i for i in range(min(len(fa), len(fo))) if fa[i] != fo[i]]
            span = f"{diff[0]}..{diff[-1]}" if diff else "none"
            print(f"  changing the {label}: {len(diff)} bytes differ, offsets {span}")

    # Where does each encrypted byte's variation come from? Classifying every
    # offset by what makes it change decides the baked-in-secret question.
    if a and b and c and d and e and f:
        bufs = {k: unhex(results[f"d{k}.add"]["file_hex"])
                for k in "abcdef"}
        L = min(len(v) for v in bufs.values())

        def diffset(x, y):
            return {i for i in range(L) if bufs[x][i] != bufs[y][i]}

        salt = diffset("a", "b")          # same pw, same second
        admin = diffset("a", "c")
        user = diffset("a", "d")
        tvar = diffset("a", "e") | diffset("a", "f")
        varying = salt | admin | user | tvar
        constant = set(range(L)) - varying
        print("\n-- byte-origin classification (over the common "
              f"{L} bytes) --")
        print(f"  vary with a random per-dictionary nonce (a vs b, same pw/second): "
              f"{len(salt)}")
        print(f"  vary with the admin password: {len(admin)}")
        print(f"  vary with the user password: {len(user)}")
        print(f"  vary with creation time only: {len(tvar - admin - user)}")
        print(f"  constant across all six files: {len(constant)}")
        print("\n  verdict:")
        if salt:
            print("  - some bytes change between two dictionaries with identical "
                  "passwords created in the same second -> a random nonce is in "
                  "play; try narrowing to the same sub-second or treat it as an "
                  "unknown to recover another way.")
        else:
            print("  - no byte changes between identical same-second dictionaries "
                  "-> no random per-dictionary nonce; all variation is accounted "
                  "for by password and creation time, both knowable.")
        if constant:
            print("  - constant encrypted bytes exist: they are enciphered with a "
                  "FIXED keystream, so known plaintext there recovers that "
                  "keystream outright -- no engine-resident secret can hide behind "
                  "a byte whose ciphertext never changes.")
        print("  - the only thing this cannot settle is a secret mixed into the "
              "password/time-dependent bytes; to rule that in or out, reconstruct "
              "those bytes from the known password and create_time alone (next step).")


def main():
    if len(sys.argv) >= 2:
        analyze_crypto(sys.argv[1])
    if len(sys.argv) >= 3:
        analyze_dict(sys.argv[2])
    if len(sys.argv) >= 4:
        analyze_stream(sys.argv[3])


if __name__ == "__main__":
    main()
