# Advantage encryption: black-box findings

What black-box probing of the Advantage Client Engine (ACE 12.0 SDK `ace32.dll`)
has established about how it encrypts table records and data-dictionary
properties, and what remains open. Everything here was obtained by driving the
documented ACE API and reading the files the engine wrote — **no disassembly**,
and no disassembly-derived material was consulted (see
[REFERENCES.md](../../../docs/REFERENCES.md) "Deliberately excluded"). The point
is to decide whether encryption can be reimplemented in tflip clean-room.

> **Resolved (2026-10-16).** Both ciphers are now reimplemented clean-room and
> verified exact against the engine (`tflip_storage::crypto`). The per-record
> generator `F`, the one piece the black-box search below never cracked, was
> obtained under the project's scoped encryption-only exception to the
> no-reverse-engineering rule: `F(S0, item)` seeds an **L'Écuyer 1988 combined
> LCG** from a nonlinear packing of four consecutive `S0` bytes (`item % 253`,
> so `S0[r..r+3]` stays in range), with `item` as the second seed, read for two
> scaled outputs `(i0, j0)`. The dictionary's "key of undetermined origin" is
> just `K` — `wrap`ped out of the admin password secret — and its properties use
> the same RC4 + `F`, differing only in the key and the item number. So the two
> "remaining blockers" were one problem. The sections below are kept as the
> record of the black-box effort that pinned everything *except* `F`; where they
> say "blocker" or "undetermined", read the resolution above.
>
> See [`crates/tflip-storage/src/crypto.rs`](../../../crates/tflip-storage/src/crypto.rs)
> for the implementation and its engine-verified tests.

Short answer, as of the black-box investigation (now superseded by the
resolution above): **table encryption is proven to be RC4 with a
password-derived key**, but was not reimplementable from black-box data alone,
because the per-record starting state (the IV) is produced by a generator that
the extensive structured search below did not identify. **Dictionary** passwords
are stored reversibly under a deterministic stream cipher with a knowable IV, but
reading a foreign dictionary was gated on the same generator plus a
dictionary-level key whose origin black-box probing could not place.

## The harness

All probes build with `i686-w64-mingw32-gcc` and run under Wine against the SDK
`ace32.dll`. Substitute your own Wine prefix and the Windows path to the SDK DLL:

    WINEPREFIX=<wine prefix> \
      tools/ace/crypto/run.sh    /tmp/c 'Z:\path\to\ace32.dll'
    WINEPREFIX=<wine prefix> \
      tools/ace/crypto/search.sh /tmp/s 'Z:\path\to\ace32.dll'

| file | what it does |
| --- | --- |
| `crypto.c` | Table encryption: encrypt an ADT, write all-zero records (ciphertext == keystream), read it back **with** and **without** the password; dump the raw `.adt`. |
| `stream.c` | Table *structure*: encrypt tables of varying record width with many all-zero records; read every record's keystream to see how it is selected per record. |
| `recover_ij.c` | Fast C probe: per password (given as **hex**, so any byte sequence round-trips exactly), encrypt/append/read, then brute-recover each record's RC4 start counters `(i0,j0)` against `S0 = RC4_KSA(pw[:20])` — same deliberate encoding (raw bytes) and truncation (20) as the Python side. Emits `{"pwhex","ij":[[i0,j0],…]}`. Scales to thousands of passwords. |
| `dict.c` | Dictionary properties: create fresh unencrypted dictionaries, record the creation second, dump the raw `.add`. |
| `multiuser.c` | One dictionary with several users (known passwords) in one process. |
| `pairs.c` | Several dictionaries per process, to separate same-second from same-counter effects. |
| `run.sh` | Builds and runs `crypto`/`stream`/`dict`; `analyze.py` summarises. |
| `search.sh` | Generates a large `(password,record)→(i0,j0)` sample via `recover_ij.exe`; runs `ij_search.py`. |
| `analyze.py` | Summarises `crypto`/`stream`/`dict` output (incl. the RC4 check and dict byte-origin classification). |
| `ij_search.py` | `localize` (per-S0-position marginal association), `search` (structured generator search: modulus sweep, wrapped+contiguous indexing, single/two-instance/combined generators, bit-slice+scaled outputs, bit-packing) and `selftest`/`selftest2` (positive controls: `selftest2` plants a combined generator with a scaled output and a contiguous non-256 window, confirming `search` recovers the new paths too) for the `(i0,j0)` generator. |
| `stream_id.py` | Identifies the generator from the `(i0,j0)` *output stream* itself (seed-derivation-independent): `stats` (representation, independence, autocorrelation, lattice/spectral tests) and `modelA` (truncated-sequence lattice reconstruction testing a continuous LCG run, with a synthetic-drand48 positive control). |

## Table encryption — what is proven

1. **The API hands out the ciphertext.** A table opened *without* the password
   opens cleanly (`AdsIsTableEncrypted` = 1) and `AdsGetRecord` / `AdsGetField`
   return the encrypted bytes rather than failing. So a chosen-plaintext oracle
   is available through the documented API alone; the raw `.adt` agrees. Writing
   all-zero records makes the returned ciphertext equal to the keystream.

2. **The cipher is RC4.** Each record's full keystream is reproduced exactly by
   `RC4_PRGA(S0, i0, j0)` for the **pristine** permutation `S0 = RC4_KSA(key)` and
   some per-record starting counters `(i0, j0)`. Verified full-length for all
   records of a table against the single `S0` (so the permutation is never mutated
   between records). `(i0,j0)` are found by brute over the 256×256 start space;
   recovery is exact (0 errors across the datasets used here).

3. **The key is the password, truncated to 20 bytes.** `RC4_KSA(password)`
   reproduces the keystream for password lengths 1–20 and fails at 21+, matching
   when the password is truncated to 20 — consistent with Advantage's documented
   20-character encryption-password limit. The repeated-character collapse
   (`A` = `AAAA` = `AAAAAAAA` give one keystream, while `aaaaaaaa`, `AAAAAAAAB`,
   `password`/`passwore` differ) is the signature of the textbook `key[i mod len]`
   schedule. There is no baked-in secret in the key and no per-record salt in the
   key: the record number does not enter the key schedule (one shared `S0`).
   The "raw bytes" encoding holds for **non-ASCII** passwords too: in the diverse
   sample 292 of 300 passwords contain bytes `0x80..0xFF` and all recover exactly
   against `RC4_KSA(raw_bytes[:20])`, so the engine keys on the raw bytes with no
   code-page translation (at least for single-byte values `0x01..0xFF`).

4. **The five-byte record prefix** (`06 00 00 00 00`) is left in the clear; only
   the fixed-length data area is enciphered.

So the only unknown for table decryption is the function that produces the
per-record IV:

> **`(i0, j0) = F(S0, record_number)`** — deterministic, password-dependent via
> `S0`, pseudorandom in the record number, re-computed per record.

## The `(i0,j0)` generator — the former blocker (now solved; see the resolution at the top)

`F` is required to read *any* encrypted record (even with the password), so it is
the single thing standing between "proven RC4" and "reimplementable." It has
resisted a long, structured black-box identification effort.

### What is known about `F`

- **Re-seeded per record (model B), not one continuous run (model A).** `F` is
  not the output-pairs nor the `(i,j)`-state of a continuous `RC4(S0)` run at any
  stride/offset (swept to 2000), and the carried-state test fails: `(i,j)_r` is
  not reachable from `(i,j)_{r-1}` by any fixed number of PRGA steps.
- **Both inputs matter.** Different passwords give different `(i0,j0)` for the
  same record (so `S0` feeds in); the same password gives a fixed, pseudorandom
  sequence over the record number with no period up to several hundred records.
- **No single `S0` byte is marginally informative.** With 8000 passwords, the
  per-position association (Cramér's V) between every `S0[c]` and `(i0,j0)` sits
  at the noise floor (`≈ sqrt(15/n)`), with peaks random and inconsistent across
  records. *This does not prove a wide seed* — a mixing function of even a few
  bytes also flattens single-byte marginals — but it does mean statistical
  localization cannot find the seed; whole constructions must be tested instead.

### What `F` is NOT (all by exact prediction, held-out validated)

- not a single `S0`/`inv(S0)` byte, nor two direct `S0[linear(r)]` bytes
  (per-record min-violations never reach 0);
- not a nested `S0` index (`S0[S0[r]]`, `S0[a·S0[r]+b]`, …);
- not a fixed keystream position of `RC4(S0)`, nor `keystream_byte XOR f(r)`;
- not an RC4 control-stream over any of nine password-derived keys (pairs/split
  layouts, drops 0–40);
- not `hash(seed ‖ recno)` for MD5/SHA-1/SHA-256/CRC-32 over several seeds;
- not a 16-bit state machine (forced `(i0,j0)` collisions have different
  successors), nor an LCG on the 16-bit state;
- **not any construction in the structured search below.**

### The structured search (`ij_search.py search`)

Seed = a record-selected subset of `S0`, fed to a published generator, read for
two outputs per record; accepted only if it predicts `(i0,j0)` exactly on
held-out passwords **and** held-out records. Validation uses a 300-password ×
300-record sample, so held-out records reach index 299 — well past 256, exercising
any index wrap for real rather than only in principle. The passwords are **diverse**
— random length 1–30 (so ~⅓ exceed the 20-byte key-truncation boundary) over the
full byte range `0x01..0xFF` (mostly non-ASCII) — so the `S0` draws are independent
rather than sharing a long common KSA prefix, and `recover_ij.c` and the Python
side agree on the encoding (raw bytes) and truncation (20); the whole diverse
sample recovers with **0 unrecovered** `(i0,j0)`.

The current space (15,811,200 whole-construction configurations):

- **index** — positions `((A·r + B) + step·k) mod M` with the modulus `M` swept over
  `{256,255,253,251,250,240,233,199,192,149,131,128,127,64,32}`;
  `A ∈ {1,2,3,5,7,11,16,31}`, `B ∈ {0,1,7}`, widths 1–4, step 1/2; in **two layouts**
  — *wrap* (window may wrap the array) and *contig* (base reduced so the whole
  window stays in `[0,255]` with no per-byte re-reduction, so a genuine contiguous
  window into the **top** of `S0`, positions 253–255, is actually read);
- **generators** — single-seed LCGs (MSVC/Borland/minstd×2/ANSI-C); two-instance
  families with the record number as the second seed (multiply-with-carry,
  subtract-with-borrow, multiplicative-combine, and one LCG combined with a
  record-seeded copy of itself); and **genuine multi-component combined generators
  with distinct constants per component** — L'Ecuyer two-LCG and Wichmann-Hill
  three-LCG;
- **seed packing** — byte order (LE/BE) and bit-level packings (nibble-pack,
  xor-fold, sum-mod); record optionally mixed in (none / add / xor);
- **output map** — bit-slice `(out >> s) & 255` **and scaled** `floor(out·256/m)`,
  which is a real fractional-output map (not a bit-slice) for the non-power-of-two
  moduli of minstd and the combined generators — motivated by `stats` showing the
  output is a full-range uniform byte.

**Result: 0 hits.** Two **positive controls** back the negative, both planting a
known construction into synthetic `(i0,j0)` over the real `S0` and confirming
`search` recovers it out-of-sample:

- `selftest` (simple) — a single LCG (`lcg:msvc`), bit-slice output, width-1 index
  `(2r+1) mod 251`: recovered (14 aliased configs, all held-out-correct).
- `selftest2` (hard) — exercises **this round's new paths together**: the L'Ecuyer
  two-LCG combined generator (its two components seeded from the two distinct
  inputs — the `S0` window value and the record number), the **scaled** output map
  `floor(out·256/m)`, and a **contiguous 2-byte** window at a **non-256** modulus
  (`mod 251`): recovered.

So the zero-hit result means the generator is absent from the space, not that the
harness is powerless — and that holds down to the fine construction details
(combined generator, scaled output, contiguous non-256 window), not just the easy
paths.

Scope of the negative, stated precisely: *exhausted over {bit-slice and scaled
outputs} × {single-component, two-instance and genuine multi-component generators}
× {wrapped and contiguous indexing} × the modulus sweep × the bit-packings above.*
It is **not** an unqualified "exhausted over all constructions."

### The output-stream angle (`stream_id.py`)

A second, independent attack: ignore the seed derivation entirely and try to
identify the generator from the **`(i0,j0)` output stream's own structure**. Over
the 300×300 sample:

- **Output representation** — both channels are full-range, uniform bytes (all 256
  values, χ² ≈ 272/296 at df 255); no missing residues. So the output is a byte
  slice / scaled fractional, not a small-modulus residue.
- **Independence** — `i0 ⟂ j0` within a record and across records: every pairwise
  correlation `|r| < 0.008` and mutual information sits at the noise floor
  (`I(i0;j0) = 0.0016` bits, cross-record ≈ 0.002). `j0` is *not* a function of
  `i0` (up to 211 distinct `j0` per `i0`), so these are not consecutive low-bytes
  of one LCG.
- **No serial structure** — interleaved-stream autocorrelation is flat to lag 32
  (max `|r| = 0.007`, floor 0.04); the bit-0/1/2 planes show no period-2, so these
  are not the low bits of a power-of-two LCG.
- **No lattice / spectral structure** — pooled consecutive pairs (16×16, 32×32) and
  triples (8³) are uniform (`z` ≈ −1.2…+0.1); no 2D/3D banding, no DFT spike. No
  coarse LCG/LFSR leaves a trace.
- **Continuous-run (model A) excluded for named LCGs, exactly.** Treating each
  password's stream as consecutive outputs of one LCG seeded per password, a
  truncated-sequence lattice reconstruction (LLL + Babai, `modelA`) recovers the
  seed for **0** of the sampled passwords under drand48/java, glibc, MSVC and
  Borland — for both 2-outputs- and 1-output-per-record. A **positive control**
  (synthetic drand48: 5/5 states recovered from 12 top bytes, held-out correct)
  proves the solver works, so these are real exclusions. This complements the
  earlier RC4 continuous-run exclusion.

What this leaves is a hard information limit, not a near miss: if the generator is
**re-seeded per record** (model B, which the earlier work favours), each record
exposes only **two 8-bit outputs from a fresh, unknown seed**. Two truncated
outputs per seed are too few to reconstruct any generator, and because the stream
carries no cross-record structure there is nothing to accumulate across records.
Stream-intrinsic identification is therefore **information-theoretically
underdetermined**; it necessarily routes back through the seed derivation
`F(S0, record#)` — the very thing the structured search has not cracked.

### Honest status and where to resume

> Superseded: `F` was identified (see the resolution at the top). It fell to
> corner 1 below — an L'Écuyer combined LCG seeded from a nonlinear packing of
> four consecutive `S0` bytes at `item % 253`. The black-box account below is why
> the structured search, which only ever tried affine index selection and scalar
> packings, missed it.

`F` is **unidentified** from two independent directions: the construction search
(predict `(i0,j0)` from `S0`) and the stream-identification angle (identify the
generator from the `(i0,j0)` values). Scope of the negatives: *the structured
search over the space above — modulus sweep, wrapped and contiguous indexing,
single/two-instance/combined generators, bit-slice and scaled outputs, bit-packing
— is exhausted*, and *the output stream is statistically indistinguishable from
random and is not a continuous run of RC4 or the named LCG families*. Neither is a
claim that `F` is "fully diffusing" or unbreakable. The corners still **not**
covered, in rough order of promise:

1. **Nonlinear or `S0`-data-dependent index selection** — the index that picks the
   seed bytes being itself read from `S0` (a double lookup, as RC4's own KSA does),
   rather than an affine `(A·r+B) mod M`. This is the most ADS-plausible gap.
2. **Exotic sources** — e.g. a KSA *intermediate* state (the permutation partway
   through the key schedule) rather than the final `S0`.
3. Failing those, **heavy cryptanalysis** of `F` as a keyed mixing function.

Until `F` is found, table encryption cannot be reimplemented clean-room, and
encrypted tables stay as they are in tflip (opened, records not decrypted — see
[KNOWN_ISSUES.md](../../../docs/KNOWN_ISSUES.md)).

## Dictionary properties

Probed with `dict.c`, `multiuser.c`, `pairs.c` (and `analyze.py`'s byte-origin
classification). The dictionary's `system.users` table stores per-user password
material; the user name and the space padding are in the **clear**, and only a
~60-byte block per user is encrypted (two 20-byte password-derived blocks plus the
20-byte raw password).

- **Deterministic, no random nonce.** Two dictionaries built with the same
  passwords **in the same second** are byte-identical, including the crypto
  region (confirmed with same-second parallel creation). There is no random
  per-dictionary salt.
- **IV = creation time (1-second resolution) + a per-record counter**, both
  knowable: the creation time is stored in the `.add` header in the clear.
  (Comparing files across a second boundary also moves plaintext header-timestamp
  bytes; use same-second controls to isolate the crypto region.)
- **The raw user password is stored reversibly** under the keystream (not
  one-way-hashed): XORing two same-second dictionaries whose user passwords differ
  in one character cancels the keystream and leaves exactly that character's
  difference at one offset. So a user password is recoverable *given the
  keystream*.
- **The key is undetermined (Q vs P).** It is not the user password (that is the
  plaintext), not the creation time (time-as-RC4-key tested, several encodings),
  and not a guessable static constant (`{empty, ADSSYS, adssys, ADM, U1,
  Advantage, iAnywhere, Sybase, default, …}`). Two models remain: **Q** — a fixed
  constant key with `(time,counter)` as IV, so `S0` is shared across all
  dictionaries; **P** — a key `g(time)` with the counter as IV, so `S0` varies per
  time. Distinguishing them needs RC4 state recovery from a long known-plaintext
  keystream, but field-level encryption yields only ~20–60 known-plaintext bytes
  per field at scattered unknown `(i0,j0)` — too little. **Undetermined.**

Either way, *assuming* the dictionary's per-field `(i0,j0)` is produced the same
way as the table's — consistent with the evidence but not separately proven —
dictionary reading inherits the table generator above. (Resolved: it is the same
`F`, and the key is `K`, `wrap`ped out of the admin password secret — see the
resolution at the top. Model Q/P was moot; the item fed through `F` is the nonce,
and the creation-time IV was a red herring.)

## Relation to the no-reverse-engineering rule

This is all black-box: inputs and outputs of the running engine, and files it
wrote. It establishes the *cipher* (public: RC4) and the *key schedule* (public:
RC4 KSA over the 20-byte password) without any disassembly. The one piece that
would complete table decryption — the IV generator `F` — is likewise being sought
black-box; nothing here uses, and nothing here should use, any disassembly-derived
notes. If `F` is found this way, a clean-room encryption
implementation becomes possible and can be verified against the engine with the
same probes.

## Permission grants — solved (2026-10-17)

`AdsDDGrantPermission(hAdminConn, usObjectType, pucObjectName, pucParentName,
pucGrantee, ulPermissions)` must be given a **dictionary connection** (connect to
the `.add`, not the folder — a folder connection answers `AE_INVALID_DICTIONARY_HANDLE`
5125), as `ADSSYS`. A grant with bits the object does not allow fails with 5158
(`AE_INVALID_OBJECT_RIGHT`); for a table only `ADS_PERMISSION_ALL` (`0xffffffff`)
and the table-valid bits (`0x333`: read, update, insert, delete, alter, drop) are
accepted.

Each grant writes (or updates) one **Permission object** (`Object Type`
`Permission`, type 21):

- **Parent** = the grantee's object id (the User or Group granted to).
- **Object Name** = the target object's id as an uppercase 8-digit hex string
  (e.g. table id 2 → `"00000002"`).
- **Info1** = the target object's id.
- **Info2** = `0x80000000`.
- **Property slots** hold the rights, enciphered: slot 1 the object rights, slot 2
  the per-column rights, slots 3/4 the same pair for the with-grant-option kind.

The rights encipherment (the missing piece earlier) is:

    blob = value_u32_LE ‖ CRC32(value)          (zlib polynomial, LE, over value)
    crypt(K, object_id + slot + 1, blob)         (RC4 under the dictionary key K)

where `object_id` is the Permission record's **own object id** (not the record
number, not the grantee), and `slot` the property-slot index (so slot 1 → item
`object_id + 2`). The trailing CRC-32 is the recognizer — a wrong key fails it.
`K` is the dictionary key recovered from the Database login secret as for group
membership. Keyed off the object id, the item (and so the keystream) differs per
permission, which is why identical grants have different ciphertext.

The stored object-rights mask: `(granted & valid_bits) | INHERIT` for a user
(a user's permission carries `ADS_PERMISSION_INHERIT 0x08`; a group's does not),
`ALL` expanding to `valid_bits`. A grant accumulates into the existing object
(OR); a revoke clears bits, keeps the object (even at zero), and additionally
writes slot 3 (the with-grant object rights) as zero. Granting the first
permission raises the dictionary version: the Database `Info1` and `VERSION` slot
2 → 3 and the version-string slot `6.2` → `7.1`.

This is implemented and verified byte-for-byte both ways (engine goldens and
through tflip's own DLL) in `tflip_engine::dictionary_write` and
`tflip_storage::crypto` (`permission_blob`, `permission_rights`,
`permission_column_blob`, `permission_column_rights`, `permission_item`).

Column grants and the grant option are written too (2026-10-17), probed against
the engine:

- A **column grant** (object type 4) stores each column's byte in the table
  Permission object's slot 2 (`[byte per field] ‖ CRC-32`, one byte per field in
  field order) and ORs the granted bits into slot 1 so the table's object-rights
  aggregate the columns. A **column revoke** drops the column's byte and lowers
  slot 1 to `inherit | (slot1 & ~inherit & ~revoked) | OR(remaining column bytes)`
  — a bit survives in slot 1 while any column still holds it (probed with two
  columns). As for a table revoke, slot 3 is written zero.
- The **grant option** (`ADS_PERMISSION_WITH_GRANT`) keeps the granted bits in
  slot 1 as usual and additionally writes slot 3, the with-grant object rights;
  a column grant with the option also writes slot 4, the with-grant per-column
  bytes (same layout as slot 2), with slot 3 their aggregate.

Every **other object type** is granted the same way (2026-10-17): the Permission
object layout is uniform (parent = grantee, `Info1` = target id, name =
`%08X` of the target, slot 1 = the object rights with a user's inherit bit). Only
the applicable rights differ — view 0x333 (as a table), procedure/function/package
EXECUTE|ALTER|DROP (0x304), link LINK_ACCESS|ALTER|DROP (0x340), user/group
ALTER|DROP (0x300), database ALTER (0x100). The database is granted by passing a
null object name (type 11); its target is the Database object. Probed and verified
both ways (committed goldens for user/group/database; a both-ways decode check for
view/procedure/function/package).

**Publications and subscriptions, and the refined rules** (2026-10-23, probed by
`tools/ace/dd/pubperm.c` → `fixtures/dictionary/pubperm.json` and the goldens
`fixtures/dictionary/ops/perm{pub,sub}*`, `revkinds`, `revnone`, `colrev*`).
Publications (type 15) and subscriptions (type 17) are granted like the other
kinds, with ALTER|DROP (0x300). Articles (type 16) have no permissions (5158 on a
grant, 5136 on a read). The probe also corrected some of the rules above, for
every kind:

- **The inherit bit is stored and grantable.** A new user Permission starts with
  INHERIT, but a grant to an existing one ORs only the granted bits (a revoked
  INHERIT is not put back). INHERIT itself may be granted and revoked to a user
  (not on a column, not to a group: 5158); `ALL` includes it for a user.
  `AdsDDGetPermissions` reports the stored bit (a user with no Permission object
  for the target holds it), and a user's groups' rights are inherited
  (`usGetInherited`, `system.effectivepermissions`) only while it holds it.
- **`ALL_WITH_GRANT` (0x8FFFFFFF)** is accepted (despite its stray bits) and is
  `ALL` with the grant option: slot 3 = the valid bits (plus INHERIT for a user).
  `ALL` (0xFFFFFFFF) carries no grant option.
- **A revoke with the WITH_GRANT bit revokes only the grant option** (slot 3, and
  for a column slot 4); the rights stay. A plain revoke clears the bits in slot 1
  *and* slot 3; `ALL` clears everything. Slot 3 is written by a revoke only on a
  table or column, or when already set — so the earlier "a revoke writes slot 3
  as zero" held for tables only. A per-column slot (2 or 4) left all zero is
  dropped from the record.
- **A revoke with nothing granted still makes the Permission object**: slot 1 =
  the user's INHERIT less the revoked bits (0 for a group), slot 3 = 0 only on a
  table or column.
- **Errors**: no rights (0, or WITH_GRANT alone) 5158; `ADS_PERMISSION_CREATE`
  alone 5132 (5136 on the database), CREATE mixed with others 5158; a right the
  type does not allow 5158 (checked before the object name: an unknown object with
  a bad right is 5158); a user or group granted rights on itself 5158; an object
  type with no permissions 5158; unknown object or grantee 5132. Object type 0 is
  accepted and resolves the name across kinds (see below).
- **SQL**: `GRANT ALTER ON pub TO bob` grants on the publication (the unqualified
  name is resolved across kinds); `GRANT … ON PUBLICATION …` / `ON SUBSCRIPTION …`
  is a syntax error (7200, native 2115).
- **`system.permissions`** lists publications then subscriptions after the links
  and before the users (Alter and Drop columns), each with the `PUBLICATION` /
  `SUBSCRIPTION` create rows as before; `DB:Admin` holds them at level 2.

**SQL `GRANT`/`REVOKE`, object type 0, the rights to create** (2026-10-23,
probed by [`tools/ace/dd/grants.c`](../dd/grants.c) running
[`grants.txt`](../dd/grants.txt), `fixtures/dictionary/grants.json`):

- **Type 0** looks the name up among tables, views, procedures, functions,
  packages, links, publications, subscriptions, users and groups — never columns
  or the database (5137 "The given object was not found."). Names are unique
  across those kinds (creating a second object of any kind with a used name is
  5132), so no kind has to win. A built-in group's name is 5158 ("Database roles
  cannot be modified or dropped"). A null name is 5132 before anything is
  touched; an empty one 5137.
- **SQL** is one `AdsDDGrantPermission`/`AdsDDRevokePermission` per permission,
  in order, stopping at the first failure (those before it stay): a right on the
  `ON` object is type 0, `SELECT(col)`/`INSERT(col)`/`UPDATE(col)` type 4 with
  the object as parent (a table only), `ALL` is `ALL` or `ALL_WITH_GRANT`, `ALTER
  DATABASE`/`INHERIT DATABASE` type 11, `[INHERIT] CREATE kind` the create right
  below; `WITH GRANT` / `GRANT OPTION FOR` add `WITH_GRANT`. One grantee only, no
  `TO PUBLIC`, `WITH GRANT OPTION` is 2117. A database-wide permission with `ON`
  is 2103 "Not supported"; object permissions without `ON` are 2115 unless a
  database-wide one is there too, when they run with a null name (5132).
- **Rights to create** (`ADS_PERMISSION_CREATE`, no object name, type = the
  kind) live in the grantee's own User/Group object, **slot 6**: 18 bytes and a
  CRC-32, enciphered at item `id + 7` like a Permission object's column rights.
  Byte = the kind's type number, except functions (18) at 7 and packages (19) at
  4. Bits: `0x80` create, `0x40` create with the grant option, `0x08` the user's
  inherit bit, `0x20` the inherit bit "with grant". A user's bytes start `0x08`,
  a group's `0x00`; the slot is written by any change, a revoke of nothing
  included. `CREATE | INHERIT` (`INHERIT CREATE kind`) works on the inherit bit
  instead. A revoke clears both bits of the pair, with the grant option only the
  second. `system.permissions` create rows: Create 1 (`0x80`) or 2 (`0x40`),
  Inherit from `0x08`.
- **Per-column rights** are one byte per field (read `0x01`, update `0x02`,
  insert **`0x04`**), kept in slot 2 (slot 4 with the grant option) only while the
  columns differ; a table-wide grant or revoke reaches every column, and slot 1
  (slot 3) holds every right any column holds.
- **Version**: every grant or revoke raises the dictionary to version 3 (and
  rewrites the index), even one that fails — but not one refused for a null
  name.
- **`system.permissions`** reports a right held with the grant option as 2.
- **`DB:Public`** grants are stored in the object itself (table slot 12, view
  slot 5): 32 bytes enciphered at item `id + slot + 1`, deciphering to
  `[4 bytes ?][u32 1][8 zero][u32 rights][8 zero][4 bytes ?]`. The outer eight
  bytes differ between dictionaries and objects for the same rights and are not
  a plain CRC-32/CRC-32C/Adler/MD5/SHA of the parts tried; flipping any bit
  makes the engine answer 5127. Not reimplemented.

The key `K` is recovered from the Database object's login secret with the
**connection's administrator password** (2026-10-17), not only the empty default:
`dict_key(secret, password)` succeeds exactly when `secret.len() == 40 +
password.len()` and the password verifies. So memberships and grants are read on a
password-protected dictionary when ADSSYS's password is on the connection.

## Reading `system.permissions` — solved (2026-10-17)

`system.permissions` is not the stored Permission objects but a full grid:
**every grantee × every object**, built in memory. Grantees are each user, then
each real group, then the six built-in groups. Objects are the dictionary's
tables, then all their columns (field order), views, top-level procedures,
functions, packages, links, users, groups (real then built-in), the Database
object, then ten create pseudo-objects (TABLE, VIEW, USER, USER GROUP, PROCEDURE,
FUNCTION, PACKAGE, LINK, PUBLICATION, SUBSCRIPTION). Procedures/functions inside a
package are not listed (only the package is).

Each right column holds a **level**: 0 none, 1 granted, 2 held implicitly. The
value comes from:

- **built-in groups** — a fixed policy, no stored grants: `DB:Admin` has level 2
  on everything except the built-in group objects, where it has Alter=1 on all but
  `DB:Public`; every built-in group has Alter=1 on its own group object (not
  `DB:Public`); `DB:Debug` has Alter=1 on procedures, functions and packages.
- **users and real groups** — level 1 where a grant sets the bit, else 0. A
  table/object right is the decrypted slot-1 mask. A column's right is its own
  byte from the table's slot-2 per-column grant, or — when there is no slot 2 —
  the table's slot-1 mask reflected to its columns.
- **`Inherit`** — only on a user's non-column rows: 1, or 0 on a built-in group
  object.

Which right columns apply per object type (others are NULL) is fixed: tables and
views carry Select/Update/Insert/Delete/Alter/Drop; columns Select/Update/Insert;
procedures/functions/packages Execute/Alter/Drop; links Access/Alter/Drop;
users/groups Alter/Drop; the database Alter; create rows Create. Verified row for
row against the engine's dumps of three dictionaries (`tflip_engine::system`).

`system.effectivepermissions` is the same grid, except a **user** also holds the
granted bits (level 1) of every group it belongs to — the engine rolls a user's
group memberships into its own rights. Non-user grantees are identical to
`system.permissions`. Verified the same way (the `effectivepermissions` dumps were
added to the fixtures; `mkfixtures.sh` now lists that system table too).

### `AdsDDGetPermissions` — solved (2026-10-17)

`AdsDDGetPermissions(hConn, grantee, usObjectType, objectName, parentName,
usGetInherited, *out)` returns a raw permission **mask** (bits, not the 0/1/2
level). Probed against the dict and props fixtures (real engine):

- `usGetInherited=0` returns the **granted** rights (the `system.permissions`
  cell logic); `=1` adds a user's groups' grants (the `effectivepermissions`
  logic). So the mask is `Σ bit for each applicable right whose level ≥ 1`.
- A **user** always carries `ADS_PERMISSION_INHERIT 0x08` in the mask, even with
  no stored grant and *including on a column* (where `system.permissions` leaves
  the Inherit column NULL) — but **not** on a built-in group object. Examples:
  `alice/customers inh=0 → 0x08`; `alice/customers inh=1 → 0x09` (inherits
  staff's READ); `alice/name(col) inh=0 → 0x08`.
- **Built-in** grantees return their policy mask: `DB:Admin/customers → 0x333`
  (the table's grantable bits), `DB:Admin/DB:Backup → 0x100` (ALTER),
  `DB:Debug/add_order → 0x100`, `DB:Public → 0x00`.

Error paths, confirmed by probing: an unknown grantee or object (including the
create pseudo-objects, which this API does not accept) is `AE_INVALID_OBJECT_NAME`
(5132); an object type with no permissions is `AE_INVALID_OBJECT_TYPE` (5136). A
user has the inherit bit on every object except a built-in group object (a column
included, unlike the `system.permissions` Inherit column).

Implemented in `tflip_engine::system::permissions_mask` (reusing the matrix
machinery) and exported as `AdsDDGetPermissions`; verified against the engine both
by a unit test of the probed masks and byte-for-byte through tflip's own DLL.

## AES dictionaries — solved (2026-11-03)

Probed with `aesdict.sh` on the patched engine (OpenSSL), black-box: each
secret was given a known plaintext (an ADSSYS password `admpw`, a user `u1`
with `userpw`, one `u2` with none, a group `g1`, `u1` in it, grants on a
table `t`), and the dictionaries kept are `fixtures/aes/dict` (`a2` without an
ADSSYS password, `a3` and `a4` with one, AES-128 and AES-256; `e3` encrypted
as a whole). Only what the files below confirm is kept.

- **The file is an AES table.** `CREATE DATABASE … ENCRYPTIONTYPE AES128 |
  AES256 DDPASSWORD 'p'` lays the `.add` out as an AES table: the 600-byte
  crypto block (encryption type at 0x190, nonce 0x194, IV counter 0x198,
  PBKDF2 salt 0x1A0, descriptor IV 0x1C0), the verifier at 0x38 and fields
  from record offset 13. The field descriptors stay in the clear.
- **Its records are in the clear unless `ENCRYPT TRUE`.** Without it the
  records have status 0x04 and a zero IV, header 0x4C is 0 and the flags at
  0x58 are 0x13, and the dictionary opens without the dictionary password
  (its tables then fail with 7068). With it the records are enciphered as an
  AES table's (status 0x06, a per-record IV) under the key PBKDF2 makes of the
  dictionary password, 0x4C is the type, the flags 0x17 and the `.ai` is
  enciphered too, its verifier at 0x20. Nothing then opens it without the
  dictionary password (7068, for any user and password; 7060 for a wrong
  one), and it is stored nowhere in the dictionary: `Database` slot 1 holds
  no wrapped table password (its length byte is 0).
- **The secrets inside use the RC4 constructions with AES-CTR in place of
  RC4.** `crypt(key, item)`, the `AtGdQ` obfuscation (`item = record +
  slot`) and `wrap` (`item = 1 + Σ key[i]·(i+1)`) keep their item numbers;
  each keys AES on the key text cut or zero-padded to 16 bytes in an
  AES-128 dictionary and 32 in an AES-256 one (the obfuscation too), with
  the counter block `item (u64 LE) ‖ item (u32 LE) ‖ block (u32 BE, from 1)`.
  An empty key falls back to `AtGdQ`, as for RC4. Checked: the ADSSYS and
  user secrets pass their self-check (`wrap(wrap(pw, first20), middle) ==
  pw`) and unwrap to one key `K`; `u1`'s membership deciphers to `g1`'s id
  and every grant's rights to a mask whose CRC-32 checks, in `a2`, `a3`,
  `a4` and `e3`; in the older `fixtures/aes` dictionaries, which have no
  ADSSYS password, the obfuscated `Database` slot 1 deciphers to the length
  byte 0 of an absent table password.
- **The dictionary's tables** are keyed by the dictionary password, with the
  dictionary's salt: one created after `ENCRYPT_NEW_TABLE`, or encrypted with
  `sp_ModifyTableProperty(…, 'TABLE_ENCRYPTION', 'TRUE', …)`, is armed; one
  created before is laid out for AES but unarmed.
- A probe process keeps a dictionary it opened usable by later connections:
  a connection without the dictionary password then succeeds where a fresh
  process fails with 7068. `aesdict3.txt` runs on its own for that reason.

Implemented in `tflip_storage::crypto::Scheme` and
`tflip_storage::dd::Dictionary::open_with_password`.

### Writing them (2026-11-04)

Probed with `aeswrite.sh` (`fixtures/aes/dictwrite`): three dictionaries
without an ADSSYS password — `c` keeping its records in the clear, `e`
(AES-128) and `f` (AES-256) enciphering them — then a step at a time, each
in a process of its own, the files kept after each. What the original writes:

- **A new one** is a new dictionary of the default encryption in the AES
  layout: the header marks (0x14 = 3, 0x164 = 12, the 600-byte block at
  0x170), the verifier of the dictionary password even when the records are
  in the clear, the descriptors in the clear with each field 8 bytes on,
  version 5 (`Info1` and slot 11), `10.1` as the engine it needs (slot 17)
  and the AES type as `DATA_ENCRYPTION_TYPE`; with `ENCRYPT TRUE` the type at
  0x4C, the flag 0x04 at 0x58 and the record enciphered. The IV counter is one
  past the descriptor IV, and 1000 more in one that enciphers its records.
  Its memo file is a plain dictionary's.
- **Records** it adds, changes or deletes are enciphered each under a fresh IV
  in one that encrypts them (status 0x06, 0x07 deleted), in the clear with a
  zero IV in one that does not.
- **The IV counter** in the header moves on by 1000 when a process first
  writes, and the process takes its IVs from the block (records, memo values,
  index pages alike).
- **Memo values** are stored behind an 8-byte IV in any AES dictionary,
  taking 8 more bytes of their blocks: zero and the value clear in one that
  keeps its records in the clear, fresh and the value enciphered to the end
  of its blocks in one that encrypts them (the bytes after the value are not
  zeros there, and differ between dictionaries: what a buffer held). The
  slabs' records (the allocator's chunk headers) are stored the same way,
  behind an IV, so a slab's chunks start a block later (4 blocks of 8 bytes).
- **The index** (`.ai`) of one that encrypts its records is the one of the
  clear dictionary with its tree pages enciphered (as an AES table's index
  keeps them) and its header marked: feature level 5 (0x08), the flags word
  (0x0E bit 0x10) with 0x200 (0x18), and the verifier (0x20). The index tflip
  builds anew after each step is the original's, page for page, once both
  are deciphered.
- **Its tables** are laid out for AES (0x14 stays 3 when bound), unarmed;
  after `sp_ModifyDatabase('ENCRYPT_NEW_TABLE', 'TRUE')` a new one is armed
  with the dictionary's type and salt (so its key and verifier), a nonce and
  descriptor IV of its own, its descriptors enciphered (flags 0x1C), its
  object's `Info2` 1. Even a dictionary that keeps its records in the clear
  arms them.

## Encrypting and decrypting an existing table of an AES dictionary

`aeswrite/08_encrypt` to `13_property` (`sp_EncryptTable`, `sp_DecryptTable`,
the `TABLE_ENCRYPTION` property: the same, `sp_ModifyTableProperty` or
`AdsDDSetTableProperty` 214; encrypting an encrypted table by the property is
5098 "The table is encrypted.", by the procedure nothing). A table of an
AES dictionary is laid out for AES from the start, whether it is encrypted
or not (the 600-byte block, the 8-byte IV slot after each record's prefix),
so:

- **Encrypting** arms it as a new one is armed (type, the dictionary's salt
  and so its verifier, its own nonce and descriptor IV, flags 0x1C, the
  descriptors enciphered) and enciphers each record in place, first IV the
  one after the descriptor IV. The memo values are enciphered in place, to
  the end of their blocks, the slabs' records and free runs left in the
  clear (zero IV) until they are written again. The index files are
  enciphered as a dictionary's: tree pages enciphered, the all-zero page
  (page 1) left, the tag headers (1,024 bytes, two pages of 512) in the clear,
  header marked as above. The Table object's `Info2` gets 1.
- **Decrypting** puts the descriptors and records back in the clear (status
  0x04, 0x05, IV zero, the crypto block and verifier left, 0x4C zero, flags
  0x10), and the index files as they were before (feature level 1, no
  flags bit, no verifier). Its memo values are not decrypted in place: the
  memo file forgets its free space (the fragments, the lists of free runs
  and of slabs; the end and the count of allocations stay) and then each
  value, record by record, is allocated afresh, written clear behind a zero
  IV, and its old blocks freed the usual way — so most old slabs stay
  as they were (enciphered records), the ones the usual freeing touches are
  rewritten clear.
- **The IV counter** and an IV drawn for each: the encrypted records',
  values', index pages', free runs' and slabs'.
- **A memo file of an AES table** keeps each free run's size and next run
  behind an 8-byte IV, as the slabs' records: zero and clear in a run written
  before the table was encrypted, a fresh IV and the two words enciphered
  after (what follows in the block is what a buffer held). Reading takes
  a zero IV as clear. Such a table's memo file has the IV slot from its first
  value on, encrypted or not.
- **`ENCRYPT_INDEXES`** sets its flag in the Database object (5162 in an
  RC4 dictionary with no table password) and nothing else: the index of an encrypted table
  is enciphered whether it is on or off, and the index of a table that is not
  encrypted never is.

## `AdsDDCreate` and `AdsDDCreate101`

`aeswrite/14_ddcreate`. `AdsDDCreate101`'s connect string takes
`EncryptionType=AES128|AES256` (any case) and `DDPassword` (1 to 20
characters), as `CREATE DATABASE … ENCRYPTIONTYPE … DDPASSWORD` does; RC4 or
no type makes a dictionary that is not AES and ignores a password. Refusals,
in the order they are met: an empty or unknown `EncryptionType` and an empty
`DDPassword` are 5019 (the value in the text, or `EncryptionType` or
`DDPassword`) and write nothing; a dictionary that is there is 5128, whatever
else; no `DDPassword` is 5223 and one of more than 20 characters 5005, both
after the original wrote the dictionary's header (2,800 bytes: the AES layout
with no key material, no verifier, no record, a next record number of 1) and
its memo file, which stay; a connect string key it does not know (`Encrypt`)
is 5019 with the key. `AdsDDCreate`'s `usEncrypt` is 1 for an
RC4-enciphered dictionary and takes no AES. `AdsDDCreate` of a path that
is there is 5128 as well (not 7040).

Not probed: a dictionary with an ADSSYS password written to (its secrets are
the same constructions).

## `AdsDDCreate` with `usEncrypt`

`aeswrite/22_rc4_create`. Any `usEncrypt` but 0 (1 and 2 probed) makes a
dictionary that encrypts its records with RC4. It is the dictionary of
`usEncrypt` 0 with the password check of the fixed `AtGdQ` key at header
0x38 (the word `ADVANTAGE` enciphered at item 0x45), the encryption type 3 at
0x4C and the encrypted flag at 0x58; each record's status is 6 and its data
after the 5-byte prefix is enciphered under `AtGdQ` at its slot number
(`crypto::table_record_xor`). Its index (`.ai`) is enciphered as a table's
RC4 index is: each tree page one stream under `AtGdQ` at its page number, the
orders' tag headers and the pages nothing was written to in the clear, the
file header with feature level 5, the flags word in use, the encrypted flag
(0x18) and at 0x20 the word `advantage` enciphered at item 0x2D. The memo file
of the dictionary (`.am`) holds enciphered values: each comment, description
and continuation of an object's properties is enciphered to its stored length
(the NUL a comment's length counts is part of it, a description's bare text
has none) under `AtGdQ` at the **slot** of the record that refers to it, not
its object id (a user made in a deleted view's slot is enciphered at that
slot). The tables made in it are not encrypted, and `ENCRYPT_NEW_TABLE` is
5162 as it has no table password.

## `sp_SetDDEncryptionType`

`probe: tools/ace/crypto/aesdict.c` and scratch scripts (`24_convert_refused`
replays what it refuses). The first argument is a file in the folder of a
connection to a *folder* (a dictionary connection resolves it under the
dictionary's own path, the quirk in `ADS_BUGS.md`); the second is the ADSSYS
password (a wrong or an unneeded one is 7078), the third the `DDPassword`
(AES needs one, 5019 before anything else; 21 characters or more fail with
2208 **after** the original converted the dictionary with the first 20), the
fourth `RC4`, `AES128` or `AES256` and the fifth whether the dictionary then
enciphers its records. Refused with 7068 (the path as detail) when the source is an
AES dictionary that enciphers its records and no connection of the process
gave its password; with 7060 (without the code in its text) when one did and
the third argument is another. A dictionary a process made or opened is
converted by it with any password (the original keeps it open); so the
refusals are probed in processes of their own.

What a conversion does (`aesconvert.sh`, `fixtures/aes/convert`: a dictionary
of each of five kinds, P plain, R RC4, C AES-128 keeping its records in the
clear, E AES-128 and F AES-256 enciphering them, with tables, a memo, an
index, users in groups (one in `DB:Admin`, one with a long description), a
grant to a user, to a group and to `DB:Public`, comments, a view, a long and
a short procedure, a trigger, a function, an ADSSYS password and holes
left by what was dropped; each converted to RC4, AES-128 and AES-256, with
the last argument 0 and 1). The checks come first, in this order: the ADSSYS
password (7078), the `DDPassword` (5019), then **5130** ("Cannot change
database encryption type when there are encrypted tables in the database.")
if a table of the dictionary is encrypted (its object's `Info2` bit 1), even
to the type it has; after `sp_DecryptTable` the conversion goes through.

- **The tables and their index files are left as they are.**
- **The dictionary key `K` and the ADSSYS password stay**, whatever the
  conversion (what is new in an AES dictionary is the key its dictionary
  password makes of a new salt, not `K`): every secret that `K` enciphers (a
  user's memberships, its create rights and its membership of the built-in
  groups, each permission's rights, the rights granted to `DB:Public` kept in
  the object, the wrapped table password, a link's password) is deciphered
  and enciphered again with the new cipher at the same item. The Database
  object's slot 1 is made anew (the table password re-wrapped, a fresh ADSSYS
  secret of the same password, obfuscated at record 1), its slot 0 is drawn
  anew in every conversion, even one that changes nothing else, and slot 4
  when the cipher changes.
- **A user's password is lost when the cipher changes** (RC4, AES-128 and
  AES-256 are three: RC4 to RC4, a plain dictionary to RC4 and AES-128 to
  AES-128 keep them): each user's secret is made again for the empty password
  with a fresh first block, and holds the same `K`. A user with a password
  cannot log in afterwards and can with none.
- **The version** is 5 and the engine `10.1` in a dictionary taken to an AES
  type (`Info1` and slots 11 and 17 of the Database object,
  `DATA_ENCRYPTION_TYPE` 5 or 6); one taken to RC4 from an AES one gets 4 and
  `8.1` and type 3 **whatever it holds** (an empty one, one with a user, one
  with a trigger); one that stays in the default layout keeps its own.
- **A conversion is made in steps**, each of which it uses or not:
  1. *Deciphering in place*: a dictionary that encrypts its records is put in
     the clear where it is. An RC4 one: each live record is deciphered, its
     status 6 to 4 (**a deleted record is left as it is**, whatever it was),
     the header's type (0x4C) and flag (0x58 bit 4) cleared, the password
     check at 0x38 left, each memo value deciphered where it is. An AES one:
     the same, the records' IVs zeroed, and the memo file **forgets its free
     space** (the fragments, the lists of free runs and of slabs) and takes
     each value in a place of its own, record by record, a `More Property`
     before a `Comment`, written clear behind a zero IV, the blocks it had
     freed: the memo file grows (9,216 to 23,552 bytes in the probe) and the
     old values stay in the old slabs.
  2. *The new secrets and type*, in place.
  3. *Writing anew*: the dictionary file and its memo file are written in the
     clear in the new layout from the objects, **packed into the slots from
     the first** (the deleted ones are gone, the order of the rest and every
     object id are kept; the header's next id too), each object's comment
     allocated in the new memo file before its continuation. The copy of the
     dictionary as it was after steps 1 and 2 is left as `<name>.add.BAK` and
     `<name>.am.BAK` (in the layout it had, in the clear, with the new
     secrets, the new version and the new type).
  4. *Enciphering in place*, if the last argument is 1: an RC4 one as in
     step 1 backwards (flag, password check at 0x38, type 3, each live record
     under `AtGdQ` at its slot and the memo values at the slot of the record
     that refers to them); an AES one with a new salt, nonce, IV counter and
     descriptor IV, the verifier of the dictionary password, the type, and
     each live record and each value a record refers to enciphered where it
     is under a fresh IV, **to the end of its blocks**; what the memo file
     keeps besides (the slabs' records, the free runs) stays **in the clear**,
     behind a zero IV. The index is built anew at the end, enciphered if the
     dictionary is.
- **Which steps a conversion takes**: a dictionary of the default layout (P
  or R) taken to RC4 only changes in place, by step 4 or by step 1 (no
  `.BAK`). Any other writes anew (step 3, with `.BAK`) when the layout
  changes, that is when an AES dictionary is taken to RC4 or a default one to
  AES, **and when an AES dictionary is taken to AES keeping its records in
  the clear** (last argument 0); an AES dictionary taken to AES enciphering
  its records is changed in place without `.BAK`: step 1 if its records were
  enciphered, step 2, and step 4 with a new salt, so converting to the type
  it has with the same password still writes it again (a new key and
  verifier). The deleted records of an AES dictionary taken so are left
  enciphered under the old key.
- Converting twice in a process leaves the later `.BAK` over the earlier.
- The rights granted to `DB:Public` (a table's slot 12, a view's 5, a
  procedure's 6, a function's 0) are enciphered at `permission_item(id, slot)`
  with a frame of random bytes, which the original draws again at each write
  (so does a user's membership of a built-in group, slot 8): they differ
  between the original's runs and their checksum is unknown; tflip enciphers
  the same plaintext again.
- **`sp_ModifyDatabase('DATA_ENCRYPTION_TYPE', 5 | 6 | 3)`** (`aesconvert/m_*`)
  does step 2 alone, on a dictionary of the default layout: the secrets
  enciphered again with the new cipher (the users' passwords lost, `K` kept,
  slot 4 of the Database object drawn anew), its version, engine and type as
  above (5 and `10.1`, or 4 and `8.1` back to RC4), and neither a layout
  change nor a `.BAK`; a dictionary that encrypts its records with RC4 keeps
  doing so.
- **`sp_ModifyDatabase('ENCRYPT_TABLE_PASSWORD', pw)`** sets, in the
  Database object's slot 1 (obfuscated at record 1), the table password as
  `[length][pw wrapped with K]` in front of the ADSSYS secret, which stays as
  it was; at most 20 characters of it are kept, and an empty one takes it
  away (length 0). It leaves the version as it is.
- **The same two calls on a dictionary of the AES layout** (`m_C_*`, `m_E_*`,
  `m_F_*`, and `m_E_type3x2`, `m_E_type3to6`, `m_E_tablepw_type6`,
  `m_C_type6_tablepw`, `m_P_tablepw_type5`) do the same and nothing more: the
  header's type (0x190), the memo file and the dictionary password's
  verifier stay, the objects with secrets are written again where they are
  (under a new IV if the dictionary encrypts its records; the index is
  built anew), a type the dictionary's tables have is only written into the
  Database object (one record), and the table password is wrapped with `K`
  under the cipher the dictionary has by then. RC4 given to one of the AES
  layout changes its secrets and its tables' type but not its layout, and
  RC4 given again is no change.
- **`AdsDDCreate101`'s connect string** (`aeswrite/46_connect_keys`): a key
  that is none of `Data Source`/`DataSource`, `ServerType`, `User ID`/
  `UserID`, `Password`, `TableType`, `CharType`, `LockMode`,
  `TrimTrailingSpaces`, `Compression`, `CommType`, `EncryptionPassword`,
  `EncryptionType`, `FIPS`, `DDPassword`, `Collation`, `Decimals`, `Epoch` and
  `Description` (any case; `UID`, `PWD`, `Rights`, `ShowDeleted`, `Timeout`,
  ... are not) fails with 5019 and the key as written as its detail
  (`Foo`; for an item with no `=` the item and its `;`, for an empty key the
  value and its `;`), the first unknown one, before the encryption type, the
  password and a file that is there are looked at and before a file is
  written. `AdsConnect101` refuses them the same way. `Password` is the
  new dictionary's ADSSYS password whatever the `User ID` (20 characters of it
  are kept; an empty one leaves none), and `Description` its comment.
  `sp_GetSecurityInfo`'s `DictionaryEncrypted` is true to the administrator
  of a dictionary that encrypts its records (RC4 or AES), false of one that
  does not (AES with its records in the clear too), null to anyone else; its
  `EncryptionType` is the type the dictionary had when the connection opened
  it, so a `sp_ModifyDatabase` of the type shows in the next connection.
- **Links' and subscriptions' passwords in a conversion** (`a_Y`, `a_Z`,
  `a_W`; `c_Y_*`, `c_Z_*`, `c_W_*`, `m_Y_type5`, `m_Z_*`, `m_W_type6`): the
  links (passwords of none, 6, 20 and 30 characters, with and without a
  user) and the subscriptions (none, 1, 6 and 30 characters, a long
  connection string) of a plain or an AES-128 dictionary come out of
  `sp_SetDDEncryptionType` and `sp_ModifyDatabase` to RC4, AES-128 and AES-256
  byte for byte what tflip's wrapping with `K` and its enciphering at
  `permission_item(id, slot)` make (compared deciphered): the rules for a
  link's `PASSWORD` and a subscription's `PASSWORD` and `CONNECTION_STR` were
  right. `AdsDDCreateLink` with a password and no global option (0) fails with
  5097 whatever its length; a subscription takes any password. A subscription
  makes a table of its own in the dictionary (about 21 objects), so four
  of them take a dictionary to 97 objects; there, the original's `.ai` after a
  conversion (built anew) and after `sp_ModifyDatabase` (the one key that
  changed, the Database object's `Info1`, put in the index as it was) differ
  from the index tflip builds from the objects (the same keys in other pages:
  a value in the first leaf, at 0x418 of the file, is 16 where tflip's is 14,
  and the later pages are numbered in another order), which the 53-object `a_Y`/`a_Z` and the `a_W` do not
  show.

## Tables of an RC4 dictionary with a table password (2026-11-05)

Probed with `rc4tp/run.sh` (`fixtures/aes/rc4tp`; `x*` scripts drive the
AdsDD* and table calls through `rc4tp/api.c`), on a dictionary of the default
layout (`AdsDDCreate` with 0). RC4 has no random material, so what the original
wrote is compared byte for byte (the Database object's secrets aside).

- **The password.** `ENCRYPT_TABLE_PASSWORD` is refused when it is 1 to 5
  characters (5097, "The database table encryption password must be at least
  6 characters or longer.") and, before that, with 5130 ("Cannot change
  database table encryption password when there are encrypted tables in the
  database."), the same password included, while any Table object has
  `Info2` bit 1. Taking it away (an empty one) also clears
  `ENCRYPT_NEW_TABLE`, not `ENCRYPT_INDEXES`. `AdsDDGetDatabaseProperty` of
  105 is 5138 (not set).
- **The flags.** `ENCRYPT_NEW_TABLE` and `ENCRYPT_INDEXES` on are 5162 without a
  table password (off never is); the call (`sp_ModifyDatabase`,
  `AdsDDSetDatabaseProperty` 106 and 120) only sets the presence of the
  Database object's slot, which `AdsDDGetDatabaseProperty` and
  `system.dictionary` report. A new table is encrypted only when the password
  is there at its creation: with the flag left on and the password taken away
  tables are made clear.
- **A table encrypted** (made by `CREATE TABLE` or `AdsCreateTable` after
  `ENCRYPT_NEW_TABLE`, added by `AdsDDAddTable` (a free table, its records, memo
  values and index enciphered in place), or encrypted by `sp_EncryptTable` and
  the `TABLE_ENCRYPTION` property) is `AdsEnableEncryption` and
  `AdsEncryptTable` of a free table with the table password (check at 0x38,
  type 3 at 0x4C, flags 0x04, records and memo values under it) and the
  field descriptors enciphered too: flags 0x08 and the whole region from 0x190
  to the first record (its extension, 0 here, included) one stream at item
  `0x45 × fields`. Its Table object has `Info2` 1.
- **Its index files** are enciphered only with `ENCRYPT_INDEXES` on (the AES
  dictionaries do it whatever it says): each tree page one stream under the
  table password at its page number, the header as a dictionary's `.ai` has it
  (`encipher_rc4`). An index file made while the flag is off stays clear
  and is then kept current in the clear, with the flag on later too; one that
  was enciphered stays so after the flag goes off. `sp_EncryptTable` enciphers
  the files the table has, if the flag is on, `sp_DecryptTable` puts back the
  ones that are enciphered whatever it says.
- **Decrypting** (`sp_DecryptTable`, the property off) puts descriptors and
  records back in the clear, the deleted ones (status 7) excepted, which stay
  enciphered; the password check at 0x38 stays, the type goes to 0 and the
  flags lose 0x04 and 0x08.
- **Errors.** The property on an encrypted table is 5098 ("The table is
  encrypted."), off for one that is not 5163; `sp_EncryptTable` on one is
  nothing and `sp_DecryptTable` of one that is not nothing; without the
  password 5162 for either. `AdsEncryptTable` of a dictionary's table is 5096
  ("Encryption is not enabled.  Cannot encrypt table.") or, if it is
  encrypted, 5098; `AdsDecryptTable` is always 5019 ("Database table can only be
  decrypted by setting the table property in the dictionary.").
- **A restructure** puts an encrypted table in the clear for it (the `.BAK` is
  clear) and enciphers the new one after, its index files in the clear, as for
  AES (`f1`).
- **`AdsCreateTable` on a dictionary connection** adds an ADT table to the
  dictionary (5132 if the name is taken); `AdsDDAddTable` of a table of a
  dictionary is 5130 ("The table is not a free table.  It is already
  associated with a data dictionary.").
- `AdsDDGetTableProperty` of `TABLE_ENCRYPTION` (214) is 1 for an encrypted
  table, whatever the dictionary.

## DBF tables in a dictionary with `ENCRYPT_NEW_TABLE` (RC4 with a table password: `fixtures/aes/rc4tp` `y1`-`y3`, `y6`; AES: `y4`, `y5`)

Probed with `tools/ace/crypto/rc4tp/run.sh` (the `createt`, `addtablet` and
`stmttype` steps of `api.c`). tflip makes and adds these tables in the clear
(KNOWN_ISSUES); the files of `y1`, `y2`, `y3` and `y6` are the original's.

- **Which tables are enciphered.** Every DBF table `AdsCreateTable`, `CREATE
  TABLE` (under any `AdsStmtSetTableType`) and `AdsDDAddTable` put in an RC4
  dictionary whose `ENCRYPT_NEW_TABLE` is on and that has a table password:
  CDX, NTX and VFP alike, with or without records, and
  `AdsDDGetTableProperty(TABLE_ENCRYPTION)` is 1 for them. `AdsDDAddTable`
  enciphers the free table's records and memo values in place; later inserts
  encipher too.
- **The header.** Byte 0 gains 4 (0x03 to 0x07, 0x83 to 0x87, 0xF5 to 0xF7;
  a VFP table's 0x30 is 0xF8 whether or not it has memo fields). Bytes
  0x0C-0x15 (clear in a DBF table) are the password check: byte 0x0C is 3 (4
  when the table has a memo file) and bytes 0x0D-0x15 the word `ADVANTAGE`
  (`advantage` when there is a memo file) enciphered at item 0x45 under the
  table password, as an ADT table's check is (`y3`: the same for every
  structure, other bytes for another password). Nothing else of the header
  changes.
- **The records.** The first byte of a record is `E` (0x45) for a live record
  and `D` for a deleted one (`y6`), and the bytes after it are one RC4 stream
  of the table password at item = the record's number (1-based), the same
  `F(S0, item)` as an ADT table's data area (`y2`: records of `A`s decipher at
  items 1, 2, 3). A memo field's pointer (the block number as text, or four
  bytes in a VFP table) is enciphered with its record. The end-of-file byte
  0x1A stays.
- **Memo files.** An `.fpt` block keeps its 8-byte header (type, length) in the
  clear and its value is enciphered at item = the number of the record that
  refers to it, as in an ADT memo file. A `.dbt` block is the value followed by
  0x1A, the value and that first 0x1A enciphered together at that item and a
  second 0x1A after them in the clear (`y6`, `tn.dbt`).
- **Index files** were not probed (`ENCRYPT_INDEXES` is off in these).
- **AES dictionaries** take no CDX or NTX table they would encrypt
  (`ENCRYPT_NEW_TABLE` on): `AdsCreateTable`, `CREATE TABLE` (7200 around
  7168) and `AdsDDAddTable` are 7168 ("The table format does not support
  strong encryption.") and leave no file, but a name that is taken is 5132
  first (`y5`). A VFP table is made in an AES layout of its own (`y4`, decoded in
  the next section) and `AdsDDAddTable` of such a
  table that is in the dictionary already is 5098 ("Encrypted table cannot be
  added to the data dictionary in this release."). With `ENCRYPT_NEW_TABLE`
  off every type is made and added in the clear.
- **`AdsCreateTable` of a DBF table on a dictionary connection** adds the table
  to the dictionary as an ADT one does (5132 for a taken name; `y1`, `z1`).

### Full-text orders, VarChar remainders, Binary and Image values, and the record kept after `AdsDisableEncryption` (`y11` to `y14`, `fixtures/write/dbf_encryption_cached_*`)

- **`CREATE INDEX ... CONTENT` on a CDX or VFP table of a dictionary** (`y11`
  with `ENCRYPT_NEW_TABLE` and a table password, `y12` clear) records the
  Index and Key objects as `AdsDDAddTable` does for a table with such an
  order: the key expression of the order is the field's name (an ADT table's
  says `F<n>`), its options the `ADS_FTS_*` flags, the noise words a text of
  their own. The words of a memo field are read deciphered at their record's
  number, so an enciphered table's `.cdx` is the clear one's byte for byte
  (the index files stay clear, `ENCRYPT_INDEXES` being off), and `CONTAINS`
  over the memo field, after inserts, an update and a delete, finds what the
  order holds (deleted records included). A Clipper table has no such order.
- **An ADS VarChar's remainder** (`y13`, a VarChar(10) of 30 characters, in
  `.fpt` and `.dbt`) is left in the clear in the memo file; the inline part
  is in the record, which is enciphered. A VFP table's VarChar is a plain
  character field that cuts (`Data truncated`, 2102).
- **A Binary or Image value of a Clipper or FoxPro table** (`@R_BLOB@` and
  `@SIXPIC@`, a 16-byte tag with the length) keeps its tag in the clear; its
  data is one RC4 stream at the record's number from the start of the data
  (not after the tag as one stream). In a `.dbt` the byte after the data
  and the 0x1A stay clear. (A VFP table's Binary value has no tag and is an
  ordinary memo value.)
- **After `AdsDisableEncryption`** the handle's current record reads as it
  did when the handle moved to it (`dbf_encryption_cached_*`, `g1` to
  `g10`): deciphered, whether or not it was read before; going to the same
  record again leaves it; going to another record reads the file as it is,
  enciphered. The memo values of an encrypted table are refused (5096) as
  before; those of a table encrypted by `AdsEncryptRecord` read empty.
  After the password is given again the record reads as the file has it
  (enciphered, `g8`) until the handle moves. A VFP record's memo pointer
  read that way points past the end of the memo file, and the read fails
  with 7010. The same for Clipper, FoxPro and VFP tables.

## Visual FoxPro tables laid out for AES (2026-11-06; `fixtures/aes/rc4tp` `v1` to `v12`)

Probed on the patched engine with `tools/ace/crypto/rc4tp/run.sh` (`v1.txt` to
`v12.txt`; `api.c` gained `zap`, `pack`, `begin`, `commit`, `rollback` and
`encpw`), each script on a new AES dictionary (`ddpw`, AES-128; `v3` AES-256).
The ADT AES scheme turned out to be the VFP one with another place for each
piece, so the key and the counter blocks are the ones of `AES dictionaries` above.

- **Which tables.** Only a Visual FoxPro table has an AES layout. A table the
  dictionary of AES tables makes (`CREATE TABLE` under `AdsStmtSetTableType`
  4, `AdsCreateTable`) or a free one made on a connection with
  `EncryptionType=AES128`/`AES256` is laid out for AES even when it is not
  encrypted; a Clipper or FoxPro table is not (`v10`: 0x83 and 0xF5 as ever, and
  `sp_EncryptTable` of one is 5152, "The table restructure failed.  Nothing to
  be done.", `v11`).
- **The layout.** `_NullFlags` is always there (a table with nothing nullable
  and no VarChar gets it, 1 byte of flags aside) and is 8 bytes longer than the
  flags need: those 8 bytes, the last of every record, are the record's IV
  (zero while the table is not encrypted). Records are 8 bytes longer, a header
  is 32 bytes longer where `_NullFlags` was added. The version byte is 0xF9
  while the table is not encrypted (a decrypted one keeps 0xF9), 0xF8 once it is
  (as an RC4 one's). The field descriptors stay in the clear.
- **The header of an encrypted table.** Byte 0x0C is the type (5 AES-128, 6
  AES-256), bytes 0x0D-0x15 the first 9 bytes of the 20-byte verifier
  (`PBKDF2-HMAC-SHA1(password, salt, 10000)` after the key, as the ADT header's
  0x38). The crypto block is in the 263-byte back link, from its first byte: a
  1 (4 bytes), the nonce, the IV counter (8), the salt (32), the rest zero. A
  dictionary's table has the dictionary's salt (so its key and verifier) and a
  nonce and counter of its own; a free table has a salt of its own, from the
  password of `AdsEnableEncryption`. Byte 0x1C (memo and structural index)
  and the rest of the header are as in any VFP table.
- **The records.** The first byte is `E` (live) or `D` (deleted), as with RC4;
  the bytes from the second to the one before the IV are AES-CTR under the
  key, the 8-byte IV in the record's last bytes and the table's nonce, the
  counter block `IV ‖ nonce ‖ block (u32 BE, from 1)` (the same as an ADT
  record's data). Everything but the status is enciphered: the fields, a memo
  pointer (4 bytes, LE) and the null flags. A record written while the table is
  armed but not encrypted (after `AdsEnableEncryption` and before
  `AdsEncryptTable`) is in the clear with a space, a `*` and a zero IV, and
  `AdsEncryptTable` enciphers what is not yet (`v12`). The IV counter in the
  header moves on by 1,000 when a process first writes, as an ADT table's.
- **Memo files** (`.fpt`) keep the block header (type, length) in the clear
  and, after it, an 8-byte IV, then the value enciphered to the end of its
  blocks (the bytes after it are what the buffer held): a value of N bytes
  takes `ceil((16 + N) / block size)` blocks, one more than without an IV
  from 49 bytes on in 64-byte blocks. A table that is laid out for AES but not
  encrypted has the IV slot too, zero, the value in the clear and the file
  ending after the value. The `SIxMemo` signature, the free runs and the rest
  of the header are as in a plain VFP memo file, and so are the rules that
  put a value over its old blocks or at the end (`v7`), with the blocks of a
  value counted as above.
- **Index files** (`.cdx`) are never enciphered, with `ENCRYPT_INDEXES` on
  or off (`v3`).
- **`AdsDDAddTable`** of a free VFP table into a dictionary with
  `ENCRYPT_NEW_TABLE` on (`v5`) is a restructure: the table's `.dbf` and `.fpt`
  go aside as `.BAK`, and the table is written anew where it was, laid out for
  AES with its memo values stored again one record after the other, and
  enciphered (the object is encrypted, `TABLE_ENCRYPTION` 1). With
  `ENCRYPT_NEW_TABLE` off it adds the table as it is (`v10`).
- **`sp_EncryptTable`** (and `TABLE_ENCRYPTION`) of a table laid out for
  AES arms it (nonce, counter, the dictionary's salt) and enciphers its
  records and values in place; of a plain VFP table first restructures it as
  `AdsDDAddTable` does (`v11`, `.BAK` files). **`sp_DecryptTable`** deciphers
  in place: the IV of each record zero, status ` ` and `*`, each memo value
  in the clear behind a zero IV and the rest of its blocks zeros, byte 0x0C
  zero, the verifier and the crypto block left, version 0xF9 (`v6`, `v11`); a
  later insert is in the clear (a zero IV).
- **A free table on an AES connection** (`v8`, `v12`): `AdsCreateTable` and
  `CREATE TABLE` lay a VFP table out for AES, unarmed. `AdsEnableEncryption`
  arms it with a salt and nonce of its own and `AdsEncryptTable` enciphers it
  (`v8`); SQL then refuses it, 5098 "The table is encrypted.".
- **Pack, zap and ALTER TABLE** (`v7`) work as for a plain VFP table with the
  IVs: a pack and a zap rewrite the memo file from nothing (values behind new
  IVs), an `ALTER TABLE` is a restructure that puts the old files aside
  (enciphered, as they were) and arms the new table afresh.
- **Copies** (`v13`, `v14`, `v16`): `AdsCopyTable` and `SELECT … INTO` of an
  armed VFP table of an AES dictionary write a VFP table laid out for AES (0xF9,
  IV slot zero), unarmed and in the clear, the records as the source reads them;
  `AdsCopyTableContents` into an armed table enciphers as any write does.
  `SELECT … INTO` under `AdsStmtSetTableType` VFP makes a VFP table (`.dbf`) on
  any connection, laid out for AES on an AES one, even into the dictionary
  (`INTO t`; not armed). The copy of an AES ADT table is laid out for AES,
  unarmed. Without the password the source reads blank, so a copy of an armed
  free AES table is blank records; of an RC4 DBF one, `AdsCopyTable` appends the
  enciphered bytes (the status ` `, no memo pointer) and fails 5096 at its first
  memo value. `AdsRestructureTable` of an armed free table, AES or RC4, is 5096
  ("Password is required because the table may contain encrypted records.")
  and changes nothing (`v15`).
- **A handle without the password** (free, or a dictionary's table opened free;
  `fixtures/write/dbf_aes_nopassword`): an armed AES VFP record reads blank (an
  integer 0, text spaces, no memo value, `AdsIsRecordEncrypted` 0), status as it
  is; `AdsAppendRecord` is 5096 "Encryption is not enabled.  Cannot append to an
  encrypted table unless encryption is enabled.", and writing a changed record
  5096 "... Cannot update a record in an encrypted table unless encryption is
  enabled." at the write or at the move that flushes it, the record staying
  changed (a move to the same record is not one); a delete and a recall are
  accepted and change no byte.
- **The remote server** cannot make an AES dictionary (5214, no OpenSSL even
  with the patched DLLs beside it), so transactions on AES VFP tables could not
  be probed there.
- **Transactions**: the *local* engine does not roll back a change to a VFP
  table of a dictionary (an `AdsBeginTransaction`/`AdsRollbackTransaction`
  around an insert, an update and a delete left all three, in a clear one as in
  an encrypted one). What the remote server does with an AES VFP table in a
  transaction was not probed.

## `AdsDDAddTable` of a table that is in a dictionary already (`z1`, `z2`)

- The original opens the file first: a missing one is 7041. An ADT table is
  bound to its dictionary in its own header (0x14 is 2; 3 for an AES
  dictionary's table), a DBF table is not. **A bound ADT table is 5130 ("The
  table is not a free table.  It is already associated with a data
  dictionary.")** whatever the new name, by a relative or a full path (a file
  name in another case is the same file under Wine), from the same dictionary
  or another, clear or encrypted. A DBF table of a dictionary is added again
  under another name; under a name that is taken it is 5132, after the table's
  object id was taken (the dictionary's next id is one more).
- A table whose structure the connection cannot read says why first: an
  RC4-encrypted one is 5138 ("... that property was not set for that
  object.") on a connection of a dictionary with no table password to give (a
  clear one, an AES one), 7060 with another table password, and an AES one is
  7068 on a dictionary of RC4 tables with a table password. With the right
  password they are 5130 like the rest.

## `AdsConnect101`'s connect string keys (`fixtures/config/connect101_values.json`)

The original does not split the string at `;` before it reads a key: it reads
a key up to the next `=`, so `Foo;Bar=1` is the key `Foo;Bar` (5019, detail
`Foo;Bar`) and an item with no `=` runs to the end of the string (`Foo;Bar;`
is the detail `Foo;Bar;`). Blanks and `;`s before a key are skipped and a key's
trailing blanks cut when an `=` ends it. An `=` right after the previous `;`
(no key at all) with a value is 5019 with that value and what follows it up
to the next `=`: `=a;Foo=1;` gives `a;Foo`; behind a blank or a `;`, or with
no value, it is no item and passes. `Description` is a key whose value is not
checked. Not reproduced: `=;ReadOnly=true;` (5019, detail `=`),
` ;=x;Foo=1;` (passes) and a known key whose value runs into the next item
(`ReadOnly = true Foo=1` gives the detail `true Foo=1`).
