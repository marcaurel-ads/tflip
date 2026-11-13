# Encryption probes

## In plain terms (non-expert summary)

Advantage can scramble (encrypt) the data in a table so it can only be read with
the password. tflip needs to read those tables itself — and it has to work out
*how* the scrambling works by experiment, without looking at Advantage's internal
code (the "clean-room" rule).

This folder is that investigation:

- The **test programs** (the `.c` files) hand the real Advantage engine data we
  already know — e.g. rows of all zeros — then read back the scrambled result.
  Comparing known-in to scrambled-out reveals how the scrambler works, the way you
  could reverse-engineer a cipher wheel by typing `AAAA` and seeing what prints.
- The **analysis scripts** (the `.py` files) sift all that collected data for the
  pattern behind the scrambling.

What came out of it:

- The scrambling method, and how the password becomes the key, were **worked
  out** — and there is **no secret hidden inside Advantage** for the main table
  scrambling: the password alone (its first 20 bytes) drives it.
- **The last piece was found.** Before scrambling each row, the engine picks a
  per-row "starting position" from a small number-generator. The outside-in
  methods never identified it (millions of candidate generators tried, none
  matched), so it was obtained under the project's narrow encryption-only
  exception to the no-reverse-engineering rule. Encryption is now
  reimplemented and verified exact against the engine
  (`crates/tflip-storage/src/crypto.rs`).

What is below is the record of the black-box effort and the index to its tools;
full detail is in [FINDINGS.md](FINDINGS.md), whose opening note gives the
resolution.

## How it works (technical)

Black-box probes of how the Advantage Client Engine encrypts table records and
data-dictionary properties, so the schemes can be studied without any
disassembly (see [REFERENCES.md](../../../docs/REFERENCES.md) "Deliberately
excluded"). The probes only drive the documented ACE API and read the files the
engine writes; nothing here reads the engine's code.

**The findings — what is proven, what is excluded, and what is still open — are in
[FINDINGS.md](FINDINGS.md).** This file is just the index to the harness.

## Running

    WINEPREFIX=<wine prefix> \
      tools/ace/crypto/run.sh    <work folder> 'Z:\path\to\ace32.dll'
    WINEPREFIX=<wine prefix> \
      tools/ace/crypto/search.sh <work folder> 'Z:\path\to\ace32.dll'

`run.sh` builds and runs `crypto`/`stream`/`dict` and prints `analyze.py`'s
summary. `search.sh` generates a large `(password,record)→(i0,j0)` sample with
`recover_ij.exe` and runs `ij_search.py` (the generator search).

## Files

- `crypto.c` — table encryption: encrypt an ADT, write all-zero records (so the
  ciphertext *is* the keystream), read back *with* and *without* the password,
  dump the raw `.adt`.
- `stream.c` — table *structure*: vary the record width and record count to see
  how the keystream is selected per record.
- `recover_ij.c` — fast C probe that brute-recovers each record's RC4 start
  counters `(i0,j0)` against `S0 = RC4_KSA(pw[:20])`; takes **hex** passwords (so
  any byte sequence, incl. non-ASCII and >20 bytes, round-trips), emitting
  `{"pwhex","ij":…}`. Scales to thousands of passwords. Feeds `ij_search.py`.
- `dict.c` — dictionary properties: fresh unencrypted dictionaries, raw `.add`
  dumps, creation second recorded.
- `multiuser.c` — one dictionary with several known-password users.
- `share.c` — whose a table's encryption password is: several handles on one
  encrypted table (RC4, AES, record-level RC4, a dictionary's), with the
  password, without it and with a wrong one, and what each reads and may
  write (`fixtures/write/encryption/share.json`). Build it for 64 bits and run
  it from a folder with the patched engine's DLLs (OpenSSL for AES), on a data
  folder with the files of `fixtures/adt/dictionary_encrypted`; tables are
  opened by full path, as the local server under Wine takes a relative name
  against the connection folder's parent.
- `freeopen.c` — a dictionary's tables (RC4 and AES, encrypted or not; a
  made dictionary's encrypted CDX and ADT tables) opened as free tables: by
  `AdsOpenTable` in several ways and by SQL, alone, while the dictionary has
  them open and from a folder without it, with the codes and
  `AdsGetLastError` texts (`fixtures/write/encryption/freeopen.json`, its
  folder written `<data>`). Built and run as `share.c` is, on a data folder
  laid out as its header says.
- `encwrite.c` — the write lifecycle (`AdsEnableEncryption`,
  `AdsEncryptRecord`, `AdsEncryptTable`, `AdsDecryptTable`), dumping the raw
  files after each stage (`fixtures/write/encryption/`).
- `pairs.c` — several dictionaries per process (separates same-second from
  same-counter effects).
- `aesdict.c`, `aesdict.sh`, `aesdict*.txt` — AES data dictionaries made
  with known secrets (an ADSSYS password, users, a group, a membership,
  grants), with records in the clear and enciphered, and the logins and
  opens that work and fail with and without the dictionary password
  (`fixtures/aes/dict`; see FINDINGS.md, "AES dictionaries"). `aesdict.c`
  runs a script of connects and SQL statements; `aesdict.sh` builds it and
  runs each script in a process of its own on the patched 32-bit DLLs.
- `aeswrite.sh`, `aeswrite/*.txt` — what the original writes to AES
  dictionaries: three (records in the clear, AES-128 and AES-256 enciphering
  them) created, then a table, a user and a group, a grant, a long comment,
  a view, a user dropped and a table made after `ENCRYPT_NEW_TABLE`, each
  step in a process of its own and the files kept after it
  (`fixtures/aes/dictwrite`, which tflip's writes replay).
- `aesconvert.sh`, `aesconvert/*.txt` — what `sp_SetDDEncryptionType` does to a
  dictionary of each layout (plain, RC4, AES with its records in the clear and
  AES-128 and AES-256 enciphering them) converted to each type with the last
  argument 0 and 1, each conversion in a process of its own
  (`fixtures/aes/convert`, which tflip's conversion replays).
- `stmtpw.c`, `stmtpw.txt`, `stmtpw.sh` — an RC4 free table opened, written
  and dropped through SQL statements given its password
  (`AdsStmtSetTablePassword`), and what `AdsStmtEnableEncryption` does
  (`fixtures/write/encryption/stmtpw.json`, which tflip's statements replay).
- `run.sh`, `analyze.py` — build/run and summarise `crypto`/`stream`/`dict`.
- `search.sh`, `ij_search.py` — the `(i0,j0)` generator investigation from the
  seed side (`localize` = per-position association, `search` = construction search
  over the modulus sweep, wrapped+contiguous indexing, single/two-instance/combined
  generators, and bit-slice+scaled outputs, `selftest` = positive control that
  plants a known generator and confirms `search` recovers it). `search.sh`
  generates a diverse (varied-length, full-byte-range) password sample.
- `stream_id.py` — the same generator from the *output-stream* side
  (`stats` = representation/independence/autocorrelation/spectral tests,
  `modelA` = truncated-sequence lattice reconstruction of a continuous LCG run,
  with a synthetic-drand48 positive control).

## One-line status

Table encryption is RC4 with `S0 = RC4_KSA(password[:20])` and a per-record
pristine-`S0` keystream started at `(i0,j0) = F(S0, record#)`. The black-box
search never identified `F`; it came from the scoped exception and is
implemented in `tflip_storage::crypto`, as is the dictionary's scheme (the same
RC4 and `F` under a different key and item number). Details:
[FINDINGS.md](FINDINGS.md).
