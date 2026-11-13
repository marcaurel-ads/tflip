# fixtures/keyfilter

Small ADT tables for the `AdsGetKeyFilter` tests (made by the real engine with
`tools/ace/dd/keyfilter.c`; `ai` by `tools/ace/dd/kfmore.c`). Each but `ai`
has the fields `id Integer; code Character(8); name Character(10); val Double(15,2)`.

| Table | Indexes | Records | `AdsGetKeyFilter` on record 2 |
| --- | --- | --- | --- |
| `kf` | BYNAME(name), BYCODE(code,unique), BYID(id,unique), NAMEID(name;id,unique) | id 1–3 | `code='BBB'` (BYCODE, the first unique tag) |
| `kf3` | BYNAME(name), CID(code;id,unique) | id 1–2 | `code='BBB' AND id=2` |
| `kf4` | BYNAME(name) only | id 1–2 | error 5041 (no unique index) |
| `kf6` | AID(id,unique), ZCODE(code,unique) | id 1–2 | `id=2` (AID sorts before ZCODE) |
| `ai` | BYN(n) only, with an AutoInc field `ac` | ac 1-2 | `ac=2` (AutoInc fallback; OData `2L`) |

Record 2 of the others is `id=2, code=BBB, name=bravo, val=2.25`.
