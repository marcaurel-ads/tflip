# fixtures/write/encryption

The RC4 encryption-write lifecycle, written by the original engine
(`tools/ace/crypto/encwrite.c` under Wine, password `secret`). A table of
`D1 Character(20); N1 Integer; M1 Memo`, four records (one memo per record),
with an index that the encryption calls never touch.

| Stage | How it was made | Differs from `base` |
| --- | --- | --- |
| `base` | create + append 4 records | — (plaintext starting point) |
| `enable` | + `AdsEnableEncryption("secret")` | header 0x38 password check, 0x4C type 3 |
| `encrec` | + `AdsEncryptRecord` on record 1 | record 1 enciphered (0x06); memo untouched |
| `enctable` | + `AdsEncryptTable` | all records 0x06/0x07, memos enciphered, flag 0x04 set |
| `dectable` | + `AdsEncryptTable` + `AdsDecryptTable` | like `base` but 0x38 block kept, 0x4C=0 |
| `append` | + `AdsEncryptTable`, then a fifth record appended (`delta`, 444, `memo-delta`) | one more record, enciphered like the others |

Only the files that differ are kept: `enable`/`encrec`/`dectable` share
`base.adm`, and the index (`.adi`) is identical throughout.

`freeopen.json` is what `tools/ace/crypto/freeopen.c` saw opening a
dictionary's tables as free tables (the probe's data folder is written
`<data>`): codes and `AdsGetLastError` texts, through `AdsOpenTable` and SQL.

`share.json` is what `tools/ace/crypto/share.c` saw of handles on one
encrypted table (password `secret`): one with the password, others without
it (of the same connection and of another), one with a wrong one, one
exclusive; the same for a free AES table and a record-level RC4 one; and a
dictionary's encrypted table opened as a free table. `share.adt`/`.adm`/`.adi`
(RC4, two records, an index on `D1`) and `share_aes.adt`/`.adm` (AES-128) are
its tables as it made them, before the handles read them.
