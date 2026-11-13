# Bugs and quirks in the original engine

Things SAP's own Advantage Database Server and its client engine do that are
wrong, deliberate-but-surprising, or environmental, found while probing them to
build tflip. They are collected here because they shape how tflip is built and
tested: where the original cannot do something with its own data, or crashes,
there is no behaviour to copy and no golden to match, so tflip does the sensible
thing and verifies it another way.

This is the counterpart to the other two:

- [`KNOWN_ISSUES.md`](KNOWN_ISSUES.md) — where **tflip** falls short of the
  original (tflip's gaps).
- [`DIFFERENCES.md`](DIFFERENCES.md) — where tflip differs from the original
  **on purpose** (design choices).
- this file — where the **original**, or the environment it runs in, is at
  fault, and where a thing that looked like a fault turned out not to be.

Everything here was learnt black-box: from the documented `ace.h` API and from
calling the real engine, never by inspecting its binaries. The probes that show
each one live in [`tools/ace/`](../tools/ace). Unless an entry says otherwise,
the engine tested is the **32-bit client engine** (`ace32.dll`, the local
`AdsSetServerType(ADS_LOCAL_SERVER)` server) of SDK 12.0, under **Wine**; a few
entries are about the separate **evaluation server** build, and those say so.
Most entries were observed only under Wine; an entry that has since been
re-checked on **real Windows** (a native `ace32.dll` in a Windows VM) says so,
and the rest are marked environment-sensitive where it matters.
[`CONTRIBUTING.md`](CONTRIBUTING.md) has the method for re-checking on real
Windows and for settling the remaining `[artifact]`/`[unconfirmed]` ones.

## How entries are tagged

Each entry carries exactly one verdict:

- **[bug]** — a genuine engine defect: a wrong result, a wrong error, or a crash
  for *valid* input. Each one is backed by a positive control that rules out a
  precondition or a validation error.
- **[by-design]** — deliberate engine behaviour, or behaviour inherited from a
  standard library the engine embeds (ICU 4.0 for charset conversion, an old
  Microsoft C runtime for float↔text) or from a documented API contract.
- **[artifact]** — caused by the environment (Wine) or by a probe that did not
  meet a precondition; the engine itself is not shown to be at fault.
- **[quirk]** — consistent, surprising behaviour that is not clearly a defect;
  tflip reproduces most of these on purpose to match.
- **[unconfirmed]** — a suspected defect that has not been isolated black-box;
  the entry says what test would settle it.

`[crash]` and `[eval-server]` appear as extra descriptors, not verdicts: a crash
on invalid input is still `[by-design]`, a crash under Wine is still
`[artifact]`.

Every call below is given with the exact error **code and message**
(`AdsGetLastError` returns both) rather than the code alone, because one code
(especially `5132`) covers several unrelated conditions and only the message
tells them apart.

---

## Dictionary calls that look broken but are not

These calls were once listed here as "the engine cannot act on an object it just
wrote." Re-probed with positive controls — enumerating the object the engine
wrote, and retrying each call with the exact name that enumeration returns —
they turn out to work. The earlier failures were name-form or precondition
mistakes in the probe, not engine defects. The probe is
[`tools/ace/`](../tools/ace); the specific re-examination used a throwaway in
the same style (`audit.c`).

- **[artifact] Triggers must be addressed by their *qualified* name.**
  `AdsDDCreateTrigger(dict, "trg1", "trigtab", …)` succeeds; `AdsDDFindFirstObject`
  with `ADS_DD_QUALIFIED_TRIGGER_OBJ` (20) then returns the trigger as
  **`trigtab::trg1`**, and with that name both
  `AdsDDGetTriggerProperty(c, "trigtab::trg1", …)` and
  `AdsDDRemoveTrigger(c, "trigtab::trg1")` return **0**. The bare name `trg1`
  returns `5132` ("…There is no such named Trigger in the data dictionary." /
  "…The trigger is not part of the database.") — a name that does not resolve,
  not a lost object. Positive control: the enumeration proves the trigger
  exists and names it; the qualified call acts on it. The same holds for a
  trigger made with SQL `CREATE TRIGGER t ON … INSTEAD OF UPDATE BEGIN … END`
  (enumerates as `t::sqltrg3`, removes cleanly). The previously reported
  **crash** of `AdsDDRemoveTrigger` on a SQL-made trigger did **not reproduce**
  on the client engine: bare-name removal returns a clean `5132`, qualified-name
  removal returns `0`. (tflip removes triggers correctly;
  `crates/tflip-engine/src/dictionary_write.rs`.)

- **[artifact] Referential integrity needs the parent's primary key set first.**
  `AdsDDCreateRefIntegrity(dict, "ri1", "chi", "par", "pk", "chi", "fk", …)`
  returns `5131` ("…No primary key was found for the given table") when `par`
  has only a unique index named `pk`. Designate that index as the primary key
  first —
  `EXECUTE PROCEDURE sp_ModifyTableProperty('par','TABLE_PRIMARY_KEY','pk',…)` —
  and the very same `AdsDDCreateRefIntegrity` call returns **0**, as does
  `AdsDDCreateRefIntegrity62`; the rule then enumerates. Positive control: the
  committed `fixtures/adt/dictionary_ri` is built this way on the client engine
  by [`tools/ace/dd/ri.c`](../tools/ace/dd/ri.c)'s `build` pass, so RI creation
  demonstrably works. `5131` is a missing-precondition error, not "RI cannot be
  created." tflip now reproduces these checks: the parent/child tables and the
  child tag give `5132`, no primary key gives `5131`, and a parent tag that is
  not the primary index gives `5143`, each with the engine's message
  (`crates/tflip-engine/src/dictionary_write.rs`).

- **[artifact] [eval-server] `AdsDDCreateRefIntegrity` / `…62` do **not** crash
  the evaluation server.** The settling test was run: the 12.0 evaluation
  **server** (`ads.exe`, under Wine) was started and a dictionary with a parent
  (primary key set) and child table built on it, then a **remote**
  `AdsConnect101` (`ServerType=REMOTE;CommType=TCP_IP`) ran the RI create against
  it. Authenticated as `ADSSYS`, `AdsDDCreateRefIntegrity` returns **`rc=0`**
  (the rule is created), exactly as on the client engine; the server stays up.
  Without credentials it returns a clean `5054` ("current user permissions"),
  and `ALTER TABLE chi ADD … FOREIGN KEY` returns the same clean `7200` syntax
  rejection as the client engine (ADS does not accept that DDL form; RI is made
  through `AdsDDCreateRefIntegrity`). No fault in any case. The earlier "crash"
  was the precondition/permission errors (`5054` without credentials, `5131`
  without a primary key) mistaken for a fault.

- **[artifact] `AdsDDMoveObjectFile` works with an absolute destination.**
  `AdsDDMoveObjectFile(dict, ADS_DD_TABLE_OBJECT, "par", "Z:\…\sub", …)` with an
  absolute path to an existing directory returns **0** and moves the file;
  repeating it to the same place then returns `5020` ("…destination file already
  exists: …\sub\par.adt"), and a bare relative `"sub"` returns the same `5020`.
  The earlier blanket `5020` ("invalid path") was path-input validation, exactly
  as that entry already suspected. The call is not broken, so tflip now
  implements it (`crates/tflip-engine/src/dictionary_write.rs`): it moves the
  `.adt`, the `.adm` memo and the named index files, updates the stored paths,
  and answers the same `5132`/`5020` the engine does.

- **[unconfirmed] Links created locally never become active.** `AdsDDCreateLink`
  (alias `blink`, target a second dictionary) returns **0**, but
  `AdsDDFindFirstObject` with `ADS_DD_LINK_OBJECT` (12) finds **no** link
  (`5137`, `AE_NO_OBJECT_FOUND`), and `AdsDDGetLinkProperty`/`AdsDDModifyLink`
  return `5132` ("…There is no such named link…"). `AdsDDDropLink` with the
  *linked-DD name* returns `5132` ("…The specified link is not active."), but
  `AdsDDDropLink("blink")` with the **alias** returns **0** — so the earlier
  claim that all three link calls fail is wrong; drop works by alias. A link is
  a live connection to another database, which a local, server-less probe cannot
  activate, so the object is created "not active" and the name-keyed reads miss
  it. Whether an active link on a real server can be read and modified is not
  settled here. Settling test: create the link against a running ADS server with
  a reachable target dictionary, then enumerate and `AdsDDGetLinkProperty` it.

- **[artifact] `AdsDDModifyLink` / `AdsDDDropLink` by linked-DD path return
  5132.** Folded into the entry above: with the target never active, the
  path/name-keyed calls cannot resolve it. tflip implements modify and drop
  (`crates/tflip-engine/src/dictionary_write.rs`) and checks them by its own
  write→read round-trip.

---

## Crashes and faults

- **[by-design] [crash] A wild (never-a-handle) value or a null output pointer
  faults; a closed or zero handle does not.** `AdsDisconnect(0x999999)` — an
  integer that was never a handle — page-faults, because the engine dereferences
  it as a pointer, and so does `AdsGetHandleType(12345, …)`. But
  `AdsDisconnect` on a *closed* handle returns `5024` cleanly (see below),
  and the handle probes show zero and wrong-kind handles rejected with an
  error, so the engine validates handles it is tracking and
  only faults on an arbitrary pointer or a null output buffer
  (`AdsGetColumnPermissions` writes through / faults on a null `pvProperty`).
  That is invalid input outside the API contract, not a wrong result for valid
  data. The probes turn stdout buffering off
  (`setvbuf(stdout, NULL, _IONBF, 0)`) so output up to a fault survives — see
  [`tools/ace/errors/handles.c`](../tools/ace/errors/handles.c). tflip returns
  `AE_INVALID_POINTER` / the invalid-handle error instead of faulting.
- **[artifact] [crash] The DLL's own shutdown faults under Wine.** After a run,
  the engine's cleanup (`AdsCloseAllTables`/`AdsDisconnect` teardown)
  page-faults under Wine; the process is exiting anyway, so the code-page probe
  flushes stdout and returns without it — see
  [`tools/ace/codepage/codepage.c`](../tools/ace/codepage/codepage.c). A Wine
  teardown fault in exiting code, not an engine defect on live data.
- **[bug] `AdsFindServers` returns a malformed `50` with a NULL handle when no
  server answers; the discovery itself works.** Chased down with a boundary trace
  on **real Windows 10** (10.0.19045, native `ace32.dll` in a Windows VM; Process
  Monitor watching the OS boundary, not the engine's code). The call is **not**
  broken and does **not** fail before the network: for every valid `ulOptions`
  (`0`, `ADS_FS_MULTICAST_ONLY` 0x01, `ADS_FS_CONNECT_ALL` 0x02, and both 0x03)
  it **IP-multicasts a fixed 50-byte discovery request (`01 2a 00 …`) to
  `224.0.1.55:2989`, four retries ~0.25 s apart, every send succeeding**, then
  waits for replies. (The documented options are only those two bits; an
  undefined value like `4` is rejected up front with `5019`, "An invalid option
  was specified. Parameter 1". The documentation confirms the group/port — "the
  requests are sent on multicast address 224.0.1.55 and port number 2989" — and
  that default/`CONNECT_ALL` then do a follow-up connect to each discovered
  address to fill the detail columns.)

  Positive control that this is the "no reply" path, not a broken call: put *any*
  responder on that group. A throwaway listener that joins `224.0.1.55:2989` and
  echoes the request back (`tools/ace/findservers/responder.ps1`) flips both calls
  to **`rc=0`** with a **valid result cursor** — a 13-column table (`IP Addr`,
  `Port`, `Multicast Addr`, `Server Name`, `Serial Number`, `Version`, `Owner`,
  `Install Date`, `User Count`, `Server Type`, `Up Time`, `Operations`,
  `More Info`), zero rows only because the echo is not a valid server-info record.
  So the input is valid and the engine can build the cursor; the only variable is
  whether a reply arrived.

  The defect is how the *empty* outcome is reported. With no reply the call
  returns **`50`** and writes the output handle to **`NULL`** (`wrote_handle` is
  true, `htable` is `0x0`; `AdsGetNumFields`/`AdsGetRecordCount` on it then give
  `5023`/`5018`). `50` is **not an ADS error code** — the SDK defines no `AE_*`
  with that value (they are `0` or `≥5001`) — and `AdsGetLastError` returns `0`
  with an **empty message**, so the engine's own error system does not recognise
  it. (It is numerically the Win32 `ERROR_NOT_SUPPORTED`, but whether it is even an
  OS-passthrough code is unproven; it may be a raw internal value.) The SDK
  documentation makes this a contract violation, not a judgement call: it
  describes `phTable` only as "Returns a table handle that contains one row for
  each address discovered by the API" — one row per discovery, so zero
  discoveries is the empty table the reply path already returns — and documents no
  NULL handle and no error return for "no servers". (Its example just checks
  `== AE_SUCCESS` and uses the handle.) So a valid "no servers found" result
  should be `AE_SUCCESS` with that empty cursor — not an out-of-range,
  message-less `50` plus a NULL handle. That the *environment* has no ADS server
  is why there is nothing to find here; the malformed return code is the engine's
  own doing and would occur on any server-less network, including a real LAN with
  no ADS server up.

  With a server running, the call works exactly as documented — confirmed
  **end-to-end** against the real 12.0 evaluation server (the
  server run under Wine, setup in `LOCAL.md`; the bare
  Windows-VM copy would not serve, blocking on a trial dialog interactively and
  exiting with the server error `7045` as a service). Discovery from a Wine client
  on the same host returns `rc=0` and a populated cursor: the server
  (`Server Name` `ADSSERVER1`, `Serial Number` `3333332`, `Version` `12.0.0.0`,
  `User Count` `5`, `Server Type` `7`, `Up Time`, `Operations`, …), one row per
  address it advertises. The options behave per the docs: **default** fills the
  detail columns only for the address matching the multicast response and leaves
  the others address-only; **`ADS_FS_MULTICAST_ONLY`** returns every row
  address-only (no follow-up connect); **`ADS_FS_CONNECT_ALL`** connects to every
  address and, where that fails, puts the connect error in `More Info`
  (`Error 6097: Bad IP address or port …` for an unreachable bridge address). So
  the only defect is the empty-result path: `50` + NULL where the docs promise a
  zero-row table. A generic UDP self-test
  (`tools/ace/findservers/mcast-selftest.ps1`) also shows the VM carries multicast
  and broadcast fine, so none of this is a missing-multicast artifact. Probes:
  [`tools/ace/findservers`](../tools/ace/findservers). tflip implements the
  discovery (the address/port/name columns; details need a server connection it
  has no client for) and, unlike the original, returns an empty table rather
  than `50` + NULL when nothing answers (`docs/DIFFERENCES.md`).
- **[by-design] AES is gated behind a signed OpenSSL the stock package does not
  ship.** With the stock SDK DLL any AES operation — even opening an existing
  AES table — fails with `7160` ("Unable to find or load one of the OpenSSL
  libraries"): a deliberate product gate, not a defect. RC4 works (it is the
  engine's own cipher). A build that ships OpenSSL (`libeay32.dll` +
  `ssleay32.dll`) lifts the gate and does AES normally, which confirms it is a
  load-time gate rather than a computation fault, so AES can be probed black-box
  like RC4.

---

## Error reporting

- **[quirk] `AdsDDGetRefIntegrityProperty` of the update or delete rule
  (505, 506) with a buffer longer than 1 byte succeeds and leaves 5133**:
  it returns 0 with the one byte of the rule, but `AdsGetLastError` then
  says 5133 "Expected buffer length is 1.". tflip does the same
  (`fixtures/errors/param_n.json`).
- **[quirk] `AdsDDDeployDatabase` succeeds where it stops or skips.** A
  table whose dictionary entry has no index file but which has a production
  index (`.adi` or `.cdx`) in the destination stops the deployment, yet the
  call returns 0: the destination is left with an empty dictionary, the
  other tables are not deployed and the source dictionary is not renamed.
  An existing `backup\<file>` is not overwritten, the log says `Error
  copying`, and the call returns 0 too. tflip does the same
  ([`tools/ace/dd/deploy.c`](../tools/ace/dd/deploy.c), `drop_index`,
  `dbf_extra_index` of `fixtures/dictionary/deploy`).
- **[quirk] `AdsDDDeployDatabase` loses what it does not put back.** The
  new dictionary it makes has no built-in group memberships of a user
  (`DB:Admin`, …), no permission whose rights were revoked, and, when a
  field has a validation message, not the message, and in some
  combinations not other properties of the field either (a default,
  minimum and maximum together with a message keep only the default; with
  a message on a second field more is lost; `w_*` of
  `tools/ace/dd/deploy.c`), no minimum or maximum of a Clipper or FoxPro
  table's fields, nor a Visual FoxPro character field's. A table
  with rights granted to `DB:Public` fails the call with 5136 "Column
  level permission is not supported by the DB:Public role" and leaves a
  dictionary whose tables' records are marked deleted. With
  `usValidateOption` 2 the fail table is made anew for each constraint, so
  a table that breaks several keeps only the last one's records in it,
  though all are deleted. A stopped deployment's empty dictionary has an
  administrator secret the original made for it (58 bytes, not 41). tflip
  does the same but for the field message and DBF limit cases, which it
  refuses.
- **[quirk] `AdsDDCreateSubscription` takes the object id of a queue that
  is no file.** A queue ending in `\`, `/` or `.`, or empty, fails with
  5127 "The replication queue must be a file." but the dictionary's next
  object id has gone up by one; a queue with a folder in its name succeeds
  and makes no files, keeping a table path of its own making
  (`...\sub\q.adt`). A second subscription over the same queue name
  overwrites the first's queue files, so deleting the first is then 5159
  "The ADT table header is inconsistent with the data dictionary." tflip
  does the same (`fixtures/sql/ddrepl`).

- **[quirk] A group's rights properties succeed with an error.**
  `AdsDDGetUserGroupProperty` of ids 1001 to 1004 on a group returns 0 but
  leaves 5137 "No object matching the search condition was found in the
  data dictionary." as the last error (probed on a group with no rights
  granted). tflip does the same (`fixtures/errors/param_n.json`).
- **[quirk] A view's size detail spells "16 bit Boolean".** The 5133
  detail of `AdsDDGetViewProperty` id 5 has no hyphen where the others
  write "16-bit". tflip gives the same text (`fixtures/errors/param_n.json`).

- **[quirk] One size detail lacks its full stop.** `AdsDDGetFieldProperty`
  with a wrong buffer size for the properties 309 and 310 fails with 5133
  "The supplied buffer is not the expected size for the specified
  property.  Expected buffer length is 4", without the full stop every
  other property's detail ends with. tflip gives the same text
  (`fixtures/errors/param_n.json`).

- **[quirk] The return code and the last error disagree on a closed handle.**
  `AdsDisconnect` on a connection already disconnected returns `5024`
  (`AE_INVALID_CONNECTION_HANDLE`) while `AdsGetLastError` reports `5018`
  (`AE_INVALID_HANDLE`, "The handle given was not recognized by Advantage…").
  Both describe the same invalid-handle condition at different specificity; it is
  consistent and tflip matches it (`crates/tflip-ace/src/connection.rs`). Not a
  wrong result for valid data.
- **[by-design] The last error is sticky.** `AdsGetLastError` returns whatever
  the most recent *error-setting* call left; a call that sets no error leaves the
  previous one in place, so after a failure the "last error" cannot always be
  pinned to the call you think failed. This is the normal contract of a
  thread-last-error facility. See
  [`tools/ace/errors/handles.c`](../tools/ace/errors/handles.c).
- **[quirk] The last error after two raw setters on a connection is garbled.**
  `AdsSetTimeStampRaw` and `AdsSetFieldRaw` given a connection's handle return
  `5018` (`AE_INVALID_HANDLE`), but `AdsGetLastError`'s text then starts with a
  NUL where the `E` of "Error 5018:  The handle given was not recognized by
  Advantage. Verify specified handle is open/active.  Must be a table, cursor
  or index order handle." should be, and the length it reports runs on past the
  text into stack bytes, which differ from run to run. The code is right, and
  the other setters' texts on the same handle are clean (`AdsSetNull` gives
  that same text whole), so the probe's buffer is not at fault. tflip gives
  the text clean ([`tools/ace/write/write.c`](../tools/ace/write/write.c),
  `fixtures/write/database_type`).
- **[quirk] `AdsDDCreate101` with a `DDPassword` of 21 or more characters**
  returns 5005 but records no error (`AdsGetLastError` gives code 0 and an
  uninitialised text), and leaves the half-made dictionary behind (a
  2,800-byte `.add` and its `.am`). tflip leaves the same files
  ([`tools/ace/crypto/aesdict.c`](../tools/ace/crypto/aesdict.c),
  `fixtures/aes/dictwrite`).
- **[quirk] Some refused NULL pointers set no last error.**
  `AdsSetStringFromCodePage` given a NULL buffer returns `1502`, and
  `AdsExecuteSQLDirect`/`AdsExecuteSQLDirectW` given NULL SQL return `5021`,
  but neither records an error: `AdsGetLastError` still reports what the call
  before left. The other setters record their `5021`. Alike,
  `AdsDDGetUserProperty` with a NULL user name succeeds, and `AdsSetAOF`
  checks a NULL filter before its options while `AdsSetAOF100` checks the
  options first. tflip matches all of it
  ([`tools/ace/errors/`](../tools/ace/errors), `fixtures/errors/param_n.json`).
- **[quirk] `AdsGetField` reports length + 1 on a short buffer.** When it fails
  with `5005` (buffer too small) the engine sets `*pulLen` to the text length
  **plus one** (the NUL), not the text length. Consistent; tflip matches it.
- **[bug] A grant on a built-in group breaks later grants.**
  `AdsDDGrantPermission` with object type 9 (`ADS_DD_USER_GROUP_OBJECT`) and a
  built-in group's name (`DB:Public`, `DB:Admin`, …) is refused with 5158
  ("Privilege to system roles is not grantable.") — but after it, later grants
  on the same connection, valid ones included (`ALTER` with the grant option on
  a view, to a user), and further grants on the other built-in groups, fail
  with `5131` "The specified object ID is not a valid database object ID.
  .\cdiction.cpp, 39423"; the same grant on a fresh connection succeeds. tflip
  answers 5158 every time and leaves later grants alone. Seen with the 64-bit
  client engine (`ace64.dll`) and
  [`tools/ace/dd/grants.c`](../tools/ace/dd/grants.c) (the last steps of the
  `apirules` scenario of `grants.txt` stand after it on purpose).
- **[quirk] Granting on an unknown function blames packages.**
  `AdsDDGrantPermission` with object type 18 (`ADS_DD_FUNCTION_OBJECT`) and a
  name that is no function answers 5158 "Cannot grant or revoke permissions to
  functions in a package.", where every other kind answers 5132 "There is no
  such named … in the data dictionary."; `CREATE` with other rights is refused
  as "not valid in conjunctuion with other object permissions" (sic). tflip
  matches both, word for word.
- **[quirk] A failed BEFORE DELETE trigger is silent through the table API.**
  On a dictionary table whose BEFORE DELETE trigger raises (`RAISE nope(2,
  'keep')`), `AdsDeleteRecord` returns **0** and leaves the record in place; the
  same delete through SQL fails with `7200` (native `5177`, "Trigger execution
  failed.  Error 7200: … An exception is raised in the SQL script. {[nope] 2 :
  keep} …"). A failed BEFORE INSERT trigger through `AdsWriteRecord` does
  surface as `5177`. It mirrors the table-API RESTRICT delete of referential
  integrity, which is also silent. Consistent; tflip matches it
  ([`tools/ace/dd/constraints.c`](../tools/ace/dd/constraints.c),
  `fixtures/dictionary/constraints.json`).
- **[quirk] A set a field constraint refuses keeps the old value, and the
  record stays writable.** `AdsSetField`/`AdsSetNull`/
  `AdsSetEmpty` of a value a field constraint refuses (`5147`–`5149`) leaves the
  field as it was and the record still writable, so `AdsWriteRecord` then
  succeeds with the field's default — the refusal is only in the set's return
  code. Writing checks every field again (a default may itself break a minimum,
  which only the write reports). Consistent; tflip matches it (same probe).
- **[quirk] A child that refuses a cascade is written anyway, and a refused
  SET NULL or SET DEFAULT is silent through the table API.** Changing a
  parent's key to a value a CASCADE child's maximum refuses (`UPDATE q SET id
  = 60`, the child's key at most 50) fails with `5149` "Constraint violation
  while updating column "pid".  The column cannot be updated to a value
  greater than the maximum constraint value." (through SQL, `7200` with that
  native error) — yet the child's BEFORE UPDATE trigger fires, its `__new`
  holding the old key, as for a table-API write after a refused set (the
  previous entry); a direct `UPDATE` of the child to 60 fires no trigger. The
  parent keeps its old key, on the local server too, which has no
  transactions. Deleting a parent whose SET NULL child cannot be NULL (`5147`)
  or whose SET DEFAULT child's default is below its minimum (`5148`) fails so
  through SQL, but `AdsDeleteRecord` returns **0** and leaves the parent, as
  for a RESTRICT rule. Consistent; tflip matches both
  ([`tools/ace/dd/rules.ops`](../tools/ace/dd/rules.ops),
  `fixtures/dictionary/rules.json`).
- **[bug] Some defaults of the wrong type are taken, and fill in nonsense.**
  A dictionary default is an expression, and `AdsDDSetFieldProperty` refuses
  one whose value a field cannot hold: `1+1`, `.T.` or `STOD('20200105')` for
  an Integer field, `1+1` for a Time or TimeStamp field, `NOW()` for an
  Integer, Numeric, ShortInt or Time field, each with `5066` "The requested
  operation is not legal for the given field type.  Field: "i"". But a Date
  field takes a number or a logical value, and a Money field a timestamp, and
  an appended record then holds a value no reading of the default gives:
  `1+1` is 1900-01-01, `1.6` 1900-01-06, `1=2` 1900-01-02, and `NOW()` a
  Money of 24435114342229.9802 (`STOTS('2020-01-05 10:11:12')`
  15750504067891.2000). Positive controls: the same defaults are refused on
  the sibling types above, and a Double field given `NOW()` reads it as text
  (0). tflip takes those defaults too but leaves the fields NULL (see
  `docs/DIFFERENCES.md`). Seen with the local server under Wine.

---

- **[quirk] A library's refused transaction call is reported to its
  caller late, and some of its own are reported though done.** In the
  remote server, after an external procedure's library has a commit or
  rollback refused (5047) inside the caller's transaction, the caller's
  next `COMMIT` or `ROLLBACK` takes effect but is answered 7200 with native
  error 7207 "Unexpected error from Advantage Server Agent". Outside a
  transaction of the caller, the library's SQL `COMMIT` or `ROLLBACK` after
  its own SQL `BEGIN` takes effect and is answered a bare 5047. tflipd does
  the same ([`tools/ace/aep/`](../tools/ace/aep),
  `fixtures/sql/aeprulesremote.json`).
- **[by-design] [eval-server] The evaluation server expires.** `ads12eval.exe` refuses
  every connection with 7046 once its evaluation period is over;
  reinstalling it (the setup in `LOCAL.md`) gives a new evaluation code.

## Numbers and formatting

- **[bug] Integer `AVG` returns the sum when the average is negative.**
  Reproduced with a positive control: over a column holding `1` and `2`,
  `SELECT AVG(id)` returns `2` (the true `1.5` rounded half-to-even — correct),
  but `SELECT AVG(-id)` returns `-3`, which is exactly `SUM(-id)`, not the
  expected `-2`. Same aggregate, same data shape, only the sign differs, so this
  is a genuine defect, not a rounding-mode choice. tflip reproduces it.
  (`MONEY`/`NUMERIC` averages are truncated instead.) Confirmed byte-identical on
  real Windows 10 (10.0.19045, native `ace32.dll` in a Windows VM), so it is an
  engine defect, not a Wine artifact.
- **[bug] `WHERE YEAR(t) = n` over an order on a TimeStamp column finds no
  rows.** With an order on `t`, `YEAR(t) = 2001`, `YEAR(t) = YEAR(NOW())`
  and the same with `AND id > 0` or `ORDER BY t` return nothing though a row
  of 2001 is there. Positive controls return it: the same queries before
  the order exists, `YEAR(d)` over an order on a Date column, `MONTH(t)`,
  `HOUR(t)`, `YEAR(t) + 0 = 2001`, `YEAR(t) >= 2001`, `YEAR(t) <> 2001` and
  `CAST(t AS SQL_DATE)`. Reported on the forums for 12.0.0.2 and so there.
  tflip finds the rows (see DIFFERENCES).
- **[bug] `TRANSFORM` of a number with an integer picture and a literal
  loses the last digit.** `TRANSFORM(100, "999%")` is ` 10%`, `TRANSFORM(100,
  "@BR 999%")` is `10%` and `TRANSFORM(1234, "9%99")` is `1%23`: the number is
  formatted to the picture's whole width, literals included, the literal is
  put in at its place and the result cut to the picture's width. Positive
  control: `TRANSFORM(100, "999")` is `100`, and a picture with a point keeps
  its digits (`TRANSFORM(50, "999.99%")` is ` 50.00%`). Reported on the forums
  and still so in v12 ([`tools/ace/expr/iif.c`](../tools/ace/expr/iif.c)).
  tflip does the same, since the result shows in index keys.
- **[quirk] `CAST` of GUID text to `SQL_GUID` fails.** `CAST` of a
  valid GUID's text `AS SQL_GUID` fails with 5070 `Data length is not a valid length of GUID.`, in a select
  list and in an `INSERT` alike, and `WHERE g = CAST(… AS SQL_GUID)` matches
  no row. The same text inserted into a GUID column or compared with one
  works (positive control), so a GUID is written as text, never through
  `CAST` (`fixtures/sql/literals.json`). tflip converts the text instead.
- **[by-design] SQL reads numbers with an exponent inexactly.** `SELECT 1e30`
  gives `999999999999999880000000000000.0`: the SQL engine gathers the digits
  in a double and then multiplies (or divides) by 10 once per power, rounding
  at each step, so `1e30` becomes 9.9999999999999988e29 rather than the
  nearest double (from `1e23` on, and `123.456e0` is 123.45599999999999). The
  text of a double itself is its exact 17 digits (`AdsSetDouble(1e30)` reads
  back `1000000000000000000000000000000`); only a tie at the 18th digit is
  rounded its own way. The same text appears on real Windows 10 (native
  `ace32.dll`), so it is not a Wine artifact. tflip reads SQL numbers the same
  way (see `DIFFERENCES.md` for the ties, which tflip rounds the ordinary
  way on purpose).
- **[quirk] VFP `TimeStamp` index keys round one bit off the nearest double.** A
  VFP TimeStamp key is a double of the day and its fraction; for about a quarter
  of the values in `fixtures/write/cdx_indexes` the engine's stored key is one
  ULP below the nearest double, so the key cannot be *reconstructed* from the
  stored value by the obvious computation. The engine finds its own keys
  consistently, so this is a self-consistent rounding quirk of its key
  derivation, not a wrong result the engine suffers; tflip builds the key as
  `(day × 86400 + seconds) × (1/86400)`, which gives the stored one. See
  `crates/tflip-engine/src/table/keys.rs` and [`tools/ace/write/xindex.py`](../tools/ace/write/xindex.py).

---

## Forum reports probed

- **[bug] An apostrophe in a comment moves or drops a syntax error's
  place.** With `it's` in a `--` comment, a `/* */` comment or a trailing
  comment, the location is one character too far, and `(line: L column:
  C)` is missing (one such comment) or a line short (two). Positive
  control: the same text with `its` gives the right line and column. tflip
  gives the true place (see DIFFERENCES).
- **[bug] A partial key seek with an accented character finds nothing under
  a VFP collation.** On an ADT order made under
  `GENERAL_VFP_CI_AS_1252`, hard seeks of `Céli`, `Célin`, `Cé` and `céli`
  fail though `Céline` and `Célia` are there, and a soft seek of `Céli`
  stops on `Célia` with found false. Positive controls: the whole keys,
  `Cel`, `Celi` and `Celin`, and the same table with no collation.
- **[quirk] `sp_CreateReferentialIntegrity` over orders with a dynamic
  collation fails on a connection without it**, with 7200 (NativeError
  7008, `The specified table, memo file, or index file was unable to be
  opened.`), not the 5209 the forums report; on a connection with the
  collation it works, and so does `SELECT` on the tables from the other.
  tflip creates it on both (see DIFFERENCES).
- **[quirk] `ALTER TABLE … MEMOBLOCKSIZE` is dropped unread after two
  column actions** (`DROP COLUMN b ALTER COLUMN a aa INTEGER MEMOBLOCKSIZE
  128` keeps the block size, an invalid size too), and `IGNORE` or `RESPECT
  TRANSACTIONS` in such a statement then fails with 2240 `Cannot specify
  both IGNORE and RESPECT TRANSACTIONS`, or with 2004, by their order. A
  `MEMOBLOCKSIZE 0` is a syntax error (2115, `Expecting either "ADD",
  "ALTER", or "DROP".`). tflip does the first, not the 2240.
- **[quirk] `AdsGetAllTables` needs one slot more than there are tables**:
  with the array exactly as long, it fails with 5005 (and with a length of
  0 for no tables). `AdsGetAllLocks` and `AdsGetAllIndexes` need only as
  many; all three set `*len` to the whole count when they fail. tflip does
  the same.
- **[unconfirmed] `+SUM(0)` crashing the server (v10) and a mass delete
  breaking an ADI order on `a;DESCEND(b)` (7017, 11.10)** do not happen in
  12.0.0.2 on the local server: `+SUM(0)` is 0, and the order stayed whole
  after a partial `DELETE` of 1000 records (the full deletes left an empty
  table to check, so they show little).

## Code pages and Unicode

The engine converts single-byte and Unicode text with **ICU 4.0**, shipping the
exact data it uses (`icudt40l.dat`, and SAP's `aicu32.dll`/`aicu64.dll`). ICU is
open source, so the right oracle for these is stock ICU 4.0 with that data file,
not the Windows code-page tables — a divergence from Windows that matches ICU is
not a bug.

- **[by-design] The single-byte tables are ICU's, not Windows'.** Probed byte by
  byte ([`tools/ace/codepage`](../tools/ace/codepage)), the conversions differ
  from the Windows tables: OEM control characters `0x1A`, `0x1C` and `0x7F` are
  rotated, an undefined ANSI byte maps to the character of its own value, CP437's
  `0xE6` is U+03BC (µ, the IBM-PC Greek-mu reading ICU uses, not Windows'
  U+00B5), and CP852's `0xAA` is U+FFFD. These are ICU-4.0 conversion behaviours;
  tflip embeds the engine's tables. See `crates/tflip-ace/src/codepages.rs`.
  (Full confirmation is a byte-diff of the engine's `icudt40l.dat` against stock
  ICU 4.0's.)
- **[by-design] An unknown code page fails every byte with 1565.** A page the
  engine has no converter for returns `1565` ("Failed to open a converter…") for
  every byte — ICU's "cannot open converter" surfaced through the API. tflip
  refuses the same page the same way.
- **[by-design] Unicode↔code-page errors raise 5211** (`AE_UNICODE_CONVERSION`)
  on an OEM table when a character has no representation — the documented error
  for an unmappable character.

---

## Indexes, collations and seeking

- **[quirk] Some Unicode expression keys hold bytes of earlier keys**: an
  `IIF` whose first branch is 8-bit over NChar fields, and `BASE64ENCODE`
  of an empty NVarChar value, leave bytes of the previous key in the key,
  and `REPEAT` over text with supplementary characters gives keys that
  change between runs (`tools/ace/write/nchar_expr_more.py`; left out of
  its fixture). tflip writes zeros or the empty encoding, or refuses the
  order.
- **[by-design] `AE_INDEX_COLLATION_MISMATCH` (5175).** The engine refuses to
  open a table whose indexes were built under a different ANSI collation than the
  client's, raising `5175` — a documented safety refusal, not a defect.
- **[quirk] Skipping from a record an order has no key for** (one its `FOR`
  condition leaves out, or a deleted ADT record gone to with `AdsGotoRecord`)
  depends on the index type. In a CDX order a skip of `n` starts where the
  record's entry would be (by key, then record number) and lands on the `n`th
  key from there, as one would expect. In an ADI order the first step goes to
  the key next to that place, but each further one goes *two* keys, so a skip
  of `n` lands on the `(2n − 1)`th key (1, 3, 5, then EOF or BOF), both ways;
  from where it lands it goes one key at a time again. In an NTX order any
  skip goes to EOF (forward) or BOF (back). Consistent, and matched by tflip
  (`fixtures/write/skip_unkeyed`).
- **[bug] A VFP TimeStamp that rounds up to midnight gets a key just below
  the next day's.** A CDX key of a TimeStamp is the double `(day × 86400 +
  seconds) × (1/86400)`, its milliseconds rounded to the second. When they
  round up to 86,400 seconds the key should be the next day; instead, when
  that day's double has nothing in its low 32 bits (every day below
  2,097,152, odd days below 4,194,304, every fourth day below 8,388,608, and so
  on), the engine takes one off its high 32 bits: day 2,451,545 with
  86,399,999 ms gets the key of 2,451,544.0 and day 1 that of 2 − 2⁻²⁰, so the
  record sorts before records of the day it is on. Day 0 is spared.
  `AdsBuildKeyFromRecord` and the order agree, and tflip builds the same keys
  (`fixtures/write/vfp_timestamps`, `tflip_storage::key::vfp_timestamp`).
- **[bug] A FoxPro order on a timestamp, built on records, cannot find keys
  that end in zero bytes.** An order on an expression whose value is a
  timestamp (`CTOTS(c)`, `CREATETIMESTAMP(…)`) on a FoxPro (`ADS_CDX`) table
  has 8-byte double keys. `AdsCreateIndex61` on a table with records builds
  its leaves leaving out each key's trailing zero bytes, as for any number
  key; but the engine reads the order, and keeps it current, as if the bytes
  left out were spaces, as for text. So a built key ending in zeros reads as
  ending in spaces: a hard `AdsSeek` of `01/02/2020 12:00:00` (key
  `c142c27180000000`) returns `AE_SUCCESS` with *found* false and EOF, and
  `AdsLookupKey` says false, by text or by the double `2458851.5`. Keys it
  adds later leave out trailing spaces instead, and a built key that ends in
  a space keeps its trailing byte stored as more keys come into its leaf.
  Positive control: the same records in an order created before them (so
  kept current from the start) find every key; on a Visual FoxPro table
  both kinds of order find them. tflip builds, reads and keeps such orders
  the same way, so that the files are the original's byte for byte
  (`fixtures/write/cdx_timestamps`, the table `g.dbf`).
- **[quirk] A timestamp expression's CDX key leaves out the milliseconds.**
  An order on `CTOTS(c)`, `STOTS(…)` or `CREATETIMESTAMP(…)` keys the value
  as a VFP TimeStamp field's key (`(day × 86400 + seconds) × (1/86400)`), but
  with the milliseconds cut off, not rounded as a field's are: 23:59:59.999
  keys as 23:59:59 (`fixtures/write/cdx_timestamps`). Such an expression on a
  VFP field that takes NULL gives its keys a leading NULL byte, as the field
  alone does (`UPPER(c)`, `STR(i)` too); a `FOR` condition does not.
- **[quirk] An empty `ANSI_CHAR_SET` follows the user locale, not
  `ansi.chr`.** With `ANSI_CHAR_SET` empty the local server keeps the bytes as
  their own weights only when the user locale's language abbreviation
  (`LOCALE_SABBREVLANGNAME`, which a user can override as `sLanguage`) is `ENU`,
  `ENG` or `ENC`; under every other locale — Australian, New Zealand or Irish
  English too — it weighs each byte by its rank in Windows' sorting under the
  user locale, not by `ansi.chr`'s collation of that language (under German it
  is not `ansi.chr`'s `German`). So the same index differs between a US and an
  Australian client, and a German host's indexes open only on hosts of a
  locale that sorts alike (5175 elsewhere). The system locale plays no part.
  Settled on real Windows 10 (user and system locales set apart, and
  `sLanguage` alone set to `ENU`) and under Wine for 24 locales
  (`fixtures/collation/sysdefault.json`, `tools/ace/collation/sysdefault.c`).
- **[unconfirmed] An `ANSI_CHAR_SET` or `OEM_CHAR_SET` the engine cannot find
  stalls it.** Under Wine a name the collation file lacks (`Klingon`), one
  padded with spaces (` German `), or any name with `ansi.chr` missing makes
  the first connection hang, using a whole CPU, where `5057`/`7048` would be
  expected; likely a modal error box Wine cannot show. tflip trims the name and
  falls back to the default collation. Settle by the same `adslocal.cfg` on
  real Windows with a desktop session.
- **[quirk] `AdsExtractKey` at BOF** returns the key of the record the cursor was
  on *before*, not an empty key. Surprising cursor-state behaviour; not clearly
  wrong, and consistent.
- **[by-design] Negative numbers *are* found in an NTX order.** Once listed here
  as an unconfirmed defect — inferred from the format, that an NTX order might
  seek numeric keys as plain text and so miss negatives. The settling test was
  run on **real Windows 10** (10.0.19045, native `ace32.dll` in a Windows VM): an
  NTX numeric order over −30, −5, 0, 7, 42 walks them in that numeric order, and
  `AdsSeek` finds −5 (record 2) and −30 (record 1), with a soft seek of −10
  landing between them (record 2, not found). A CDX order over identical data
  behaves byte-for-byte the same. So the engine seeks NTX numeric keys
  numerically, negatives included — there is no defect. The probe is
  [`tools/ace/settle/ntxneg.txt`](../tools/ace/settle), driven by
  [`tools/ace/write`](../tools/ace/write).
- **[bug] An AOF's `YEAR()` found through an order drops the value's
  fraction.** `AdsSetAOF` with `YEAR(born) = 1990.5`, fully optimized through an
  order on `born` (ADI or CDX) or on `DTOS(born)`, shows the records of 1990;
  `1989.9` shows those of 1989, and through a `DTOS()` order `YEAR(born) <
  1990.5` leaves 1990 out. Evaluated on each record — the same term in
  `YEAR(born) = 1990.5 .OR. RECNO() = 99`, which is not optimized — it shows
  none. So one AOF's records depend on whether an order is open. Through a CDX
  order on the date, a year it cannot make a date of (`0`, `-1`, `10000`) shows
  the records with an empty date; through an ADI order, none. tflip copies what
  the orders find (`tflip_engine` `Table::index_bounds`). Probed in
  `fixtures/write/aofs_lookup`.
- **[bug] An AOF's `$` found through an order compares the key's bytes.**
  `AdsSetAOF` with `field $ 'text'`, fully optimized through an order on the
  field, shows the records whose *key* is in the text: a VarChar key padded
  to the field's length (`vc $ 'Bob'` shows none, `vc $ 'xBob       '` the
  `Bob`), a CICharacter key in lower case compared with the text as it is
  (`ciname $ 'xBOB       '` shows none, `'xbob       '` both `Bob`s). The same
  term evaluated on each record (`... .OR. RECNO() = 99`) shows the VarChar
  value without its blanks and compares CICharacter text without case. tflip
  copies what the orders find. Probed in `fixtures/write/aofs_more`.
- **[bug] An AOF's `ISNULL()` found through a FoxPro order finds the blank
  keys.** A FoxPro table has no NULL, and evaluated on each record
  `ISNULL(name)` holds for none; fully optimized through a CDX order it shows
  the records with a blank (or zero, `.F.`, empty date) value, as `EMPTY()`
  does; `ISNULL(name) = .F.` shows the others, where evaluated it holds for
  every record. tflip copies what the orders find. Probed in
  `fixtures/write/aofs_more` and `fixtures/write/aofs_rest`.
- **[bug] An AOF's `EMPTY() = .F.` found through a Visual FoxPro order leaves
  NULL out.** On a Visual FoxPro table whose record 3 holds NULL in every
  NULL-able field, `AdsSetAOF` with `EMPTY(name) = .F.`, fully optimized
  through an order on `name`, shows the records that are neither blank nor
  NULL; evaluated on each record (`EMPTY(name) = .F. .OR. RECNO() = 99`), it
  holds for record 3 too, as `!EMPTY(name)` does through the order (there,
  `EMPTY()` of NULL is `.F.`): so `.NOT. (EMPTY(name) = .F.)` shows the blank
  record and the NULL one. The same holds for `<> .T.` and for numbers,
  dates, logicals and `UPPER(name)`; `ISNULL(name) = .F.` and FoxPro and ADT
  tables are not affected. tflip copies what the orders find. Probed in
  `fixtures/write/aofs_rest`.
- **[bug] An AOF's comparison of a leading part of a FoxPro key with longer
  text compares the key.** Through the order `CREATE INDEX ac ON cm (a, c)`
  makes (`A+C`, `a` CHAR(5)), `a = 'ab   x'` shows the records whose key
  starts with `ab   x` (`a` is `ab`, `c` is `x`), and `a <> 'ab   x'` the
  others; evaluated on each record, `=` holds for none. `==`, `<`, `>` and
  shorter text compare as evaluating does. tflip copies what the orders find.
  Probed in `fixtures/write/aofs_more`.

---

- **[bug] A full-text order that keeps scores, rebuilt beside another
  full-text order of its index file, counts some words wrong**:
  `tools/ace/write/fts_scores` (orders `sk`, `KEEPSCORE` on `m`, and `ob` on
  `body`, then `AdsReindexFTS`) gives `gamma` of `alpha beta beta gamma` 3
  and `solo` of `solo` 2; with both orders keeping scores, `ob`'s `yone`
  (once in its record) gets `delta`'s 3. `AdsReindex` does the same, in
  whichever order the orders were made. Positive control: the same orders
  as created, and `sk` rebuilt alone in its file, count 1. tflip counts
  each word's occurrences on every rebuild (`fixtures/write/fts_rebuild`
  keeps its scored order in a file of its own).
- **[quirk] A zap right after a full-text order is rebuilt keeps the memo
  file's blocks**: `AdsReindex` or `AdsReindexFTS` of a table with a
  full-text order on a memo field, then `AdsZapTable`, leaves a memo file
  of 2,304 bytes whose free list holds every block, where a zap without the
  reindex, or after reindexing an ordinary order, truncates it to its 1,024
  bytes of header. The reindex changes neither file. Not reproduced by
  tflip, which truncates; `fixtures/write/fts_rebuild` zaps after a write.

## SQL

- **[quirk] `CREATE TABLE`'s field rules: what a free table refuses, and a
  plain `NOT NULL`.** On a free connection a column's `DEFAULT`, `CONSTRAINT
  NOT NULL`, `CONSTRAINT MAXIMUM` or `CONSTRAINT ERROR MESSAGE` fails with
  `7200` native `5160` "The field-level or record-level constraint is
  invalid.  Field level constraints are not supported on free table. Column
  name: y." for the first column with one, but a lone `CONSTRAINT MINIMUM
  '1'` passes and is dropped (`(x INTEGER CONSTRAINT MINIMUM '1', y INTEGER
  DEFAULT '2')` names `y`). On a dictionary connection the same rules on an
  `AS FREE TABLE` (a lone `MINIMUM` and a `PRIMARY KEY` too) fail with
  native `5132` "The specified database object name is not valid.  There is
  no such named table in the data dictionary." and leave no file. A plain
  `NOT NULL` (without `CONSTRAINT`) passes everywhere and is kept nowhere,
  so the dictionary's field can be NULL. A second `CONSTRAINT MAXIMUM` says
  "Invalid create options:  Redundant MAXMUM Constraint." (sic). tflip
  matches all of them (`fixtures/sql/ddtables.json`).
- **[quirk] Text under a dynamic collation compares four ways in one
  statement.** With `AdsStmtSetTableCollation` (or `AdsSetCollation`) at
  `GERMAN_VFP_CI_AS_1252`, `=`, `<` and joins on columns, and `MIN`/`MAX`,
  weigh text as the collation's keys do and then tell apart characters that
  expand (`ß` from `ss`, `ä` from `ae`: `MIN` of the two is `ss`, a join of
  `ß` with `ss` finds nothing); `GROUP BY`, `DISTINCT` and `UNION` take them
  as equal (one group of 2) and put a blank before the characters that weigh
  least (`0x7F`, `0x80`, ...), where an `ORDER BY` (a temporary index) and
  `MIN` put those first; a live cursor's `WHERE` takes `ß` as `ss` (`c =
  'ss'` finds `ß`); and two literals compare character by character, unexpanded
  (`'äa' > 'aeb'` and `'resumé' > 'resumes'` are true, while the same words
  in columns compare the other way). Which of a group's equal values `GROUP
  BY`, `DISTINCT` and `UNION` show varies: the first of a few rows, mostly the
  last of many. tflip matches all but the two literals and the group's
  values (`fixtures/sql/collation_dynamic.json`,
  `tools/ace/sql/collation_dynamic.txt`).
- **[by-design] A table whose orders have a dynamic collation opens in SQL
  only under it.** A statement whose collation (its own, else the
  connection's) is another, or none, fails with `7200` "AQE Error: State =
  HY000; NativeError = 5209; [SAP][Advantage SQL Engine][ASA] Error 5209:
  Different collations are being used, which can result in poor query
  optimization. Table collation: (none), Index collation:
  GERMAN_VFP_CI_AS_1252 Table name: ger" — a hard refusal, though the text
  speaks of optimization only. So the tables of one statement cannot differ in
  collation. tflip refuses alike.

- **[unconfirmed] A second `CONTAINS` in a select list can misread a record a
  FIXED full-text index does not hold.** On `fx` (`fixtures/sql/fts`, record 7
  `alpha echo` added after its FIXED index was built), `SELECT id, CONTAINS(m,
  'alpha'), CONTAINS(m, 'echo') FROM fx` gives record 7 `T`, `F`, while `SELECT
  id, CONTAINS(m, 'echo') FROM fx`, the columns swapped, and `CONTAINS(m,
  'charlie'), CONTAINS(m, 'echo')` all give `echo` `T` for it (a select-list
  `CONTAINS` reads the record's text). It seems to follow an earlier `CONTAINS`
  that matched the same record; tflip evaluates each one alone. Settle it with
  more records added after the index and other first words
  (`tools/ace/sql/fts.txt` has the probe).
- **[quirk] A FIXED full-text index answers only where the AOF reaches it.** The
  index is a snapshot, and the engine reads it for a `CONTAINS` its AOF
  optimizes (a live cursor's `WHERE` part under no unoptimizable `OR`, a static
  cursor's top-level restriction) and for `SCOREDISTINCT`, but reads the
  record's current text for `SCORE`, a select-list `CONTAINS`, and a `CONTAINS`
  under an `OR` with an unindexed part — so `WHERE CONTAINS(m, 'echo')` and
  `WHERE CONTAINS(m, 'echo') OR c = 'zzz'` disagree on the same record, and a
  live cursor's record count includes deleted records the index still holds.
  tflip reproduces it (`fixtures/sql/fts_plans.json`).
- **[quirk] `SHOW PLAN` drops the residual of an AOF whose orders find nothing.**
  A partly optimized AOF whose candidates are none says "Restriction is not fully
  optimized." with an empty `Unoptimized Part :` (`CONTAINS(c, 'MEDICAL') AND id =
  1`, `n = 99 AND id = 1`), as `AdsGetAOFOptLevel` does; tflip matches.

- **[quirk] Empty values of a Clipper or FoxPro table are NULL and not NULL at
  once.** Those tables have no NULL, and SQL takes an *empty* field — blank, but
  also 0, false or spaces — as NULL for `IS NULL`, `IFNULL` and `COALESCE` (so
  `WHERE n IS NULL` finds a stored 0, and `IFNULL(l, TRUE)` of a false one is
  `TRUE`), while everything else computes with its value: `COUNT` counts it,
  `AVG` divides by it, `MIN` of a blank number is 0, `n = 0` and `n + 0` see 0. A
  *blank* one shows as NULL in a result (a number, date or logical; text, a
  double or an integer shows its value) and groups apart from a stored 0, and
  `MIN` of a blank date is NULL. In a VFP table, which has NULLs, a blank date is
  `1899-12-30` in SQL results (`MIN`, `GROUP BY`, a static cursor's column) —
  yet `YEAR()` of it is NULL and a live `WHERE d = '1899-12-30'` does not find
  it. tflip matches all but the VFP corners (`fixtures/sql/dbf.json`; see
  `KNOWN_ISSUES.md`).
- **[bug] `IFNULL` of a VFP field takes an empty value for NULL.** In a VFP
  table, whose blank values are values (`IS NULL` is false, `COALESCE(n, 7)` of a
  0 is 0), `IFNULL(n, 9)` of a 0 or blank number is 9, of a false logical the
  fallback, of spaces the fallback — `IFNULL` alone tests xBase emptiness. On an
  ADT table `IFNULL` of a 0 is 0 (`tools/ace/sql/dbf.txt`). tflip gives the value
  (see `KNOWN_ISSUES.md`).
- **[quirk] The representative of a `CICHARACTER` group is engine-chosen.** Of the
  values in a group that differ only in case, which one the engine shows is
  implementation-defined and changes after a join (the last before, the first
  after). Surprising but not a defect — the group representative is not specified.
- **[by-design] Row order of equal sort keys, and join-loop order.** Rows with
  equal keys can come in different orders under some `ORDER BY`s on non-result
  expressions, and the optimizer chooses which table of a join to loop over
  first. SQL does not guarantee either, so this is by design.
- **[by-design] An unknown collation name is refused with 7147** by
  `AdsSetCollation` and `AdsStmtSetTableCollation` — input validation against the
  `adscollate.adt` repository (case-insensitive, canonicalised to the stored
  spelling, trailing blanks ignored). A dictionary connection, local or remote,
  takes the same names as a free-table one (`tools/ace/collation`), and tflip
  checks them alike. The statement call also takes `ANSI`, `OEM` and `:` with an
  ICU locale (`:en_US`, `:de_ADS_CI`), refusing a locale the engine has no
  collator for with 1564; `AdsSetCollation` refuses all of these.
- **[bug] `NEWIDSTRING`'s format repertoire differs between the expression
  engine and SQL, and the expression engine rejects its own documented `MIME`
  name.** `NEWIDSTRING(format)` writes a GUID in the named layout
  (`D`/`DELIMITED`, `B`/`BRACKETED` — in *square* brackets, `P`/`PARENTHESIS`,
  `N`/`NUMBERS`, `M` base64, `F`/`FILE` url-safe base64), case-insensitive. Two
  quirks, both probed on the VM (`tools/ace/expr`, `tools/ace/sql`):
  - The ADS reference lists the base64 format as "`M` or `MIME`", but the
    *expression* engine accepts only the single letter `M` and fails
    `NEWIDSTRING("MIME")` with 4407, while `F`/`FILE` both work — a
    documentation/engine mismatch.
  - The *SQL* scalar takes the format as an **unquoted word** (`NEWIDSTRING(M)`;
    a quoted `'M'` is 2159), **accepts** the `MIME` long name, and reports an
    unknown format as **2159**, not the expression engine's 4407. So the same
    call `NEWIDSTRING(MIME)` succeeds in SQL and fails in a filter. `REGISTRY` is
    rejected by both. tflip matches each path.
- **[bug] `sp_GetColumns` on a dictionary fails for any table pattern that
  takes in a system table.** On a dictionary connection, with the catalog `NULL`
  or `system`, a pattern that matches one of the system tables' names fails the
  whole call with 7200 / native 5018 (`[ASA] Error 5018:  The handle given was
  not recognized by Advantage. Verify specified handle is open/active.  The given
  handle is not readable.`) — so `'%'` always fails, and so do `'pa%'`
  (`packages`) and `'v_%'` (`views`), though `sp_GetTables` lists those tables
  fine. An empty catalog (`''`, the dictionary's own) or a link's name avoids
  it. tflip fails the same way (`fixtures/sql/spview.json`).
- **[bug] `sp_GetColumns` cannot describe a view that reads a `SELECT` in its
  `FROM`.** `SELECT x.code FROM (SELECT code FROM parent) x` is a valid view (it
  opens), but describing it fails the call with 7200 / native 5112 (`[ASA] The
  column number specified was invalid for the cursor.`). tflip fails the same
  way.
- **[quirk] A view column's nullability, as `sp_GetColumns` reports it.** A
  column of a table carries its own (even from the nullable side of an outer
  join); otherwise an expression is nullable when a column in it is — but `IS
  NULL`, `CASE` (with an `ELSE` or not) and `IIF` always are, `COUNT` and
  `EXISTS` never, and the other aggregates always. After `UNION` the first
  `SELECT`'s column decides. tflip reproduces it.
- **[quirk] A qualified table name is always a link.** In `FROM`, `a.b` is the
  table `b` of the link `a` (or of the dictionary at the path `a`, relative to
  the connection's dictionary's folder), never the file `a.b`: on a dictionary
  connection `SELECT * FROM parent.adt` fails to open the link `parent` (5174,
  `… <folder>\parent Table name: adt`), and on a free connection *any* qualified
  name fails with 5041 (`… all_types - link object is not supported on free
  connection.`) — so a free table cannot be named with its extension unless
  quoted (`"parent.adt"`). tflip does the same (`fixtures/sql/links.json`).
- **[quirk] Errors placed at a `.` point just past it.** The location the SQL
  engine gives for an error at a dot (`SELECT * FROM a.b.c` → 2117, `SELECT
  a.b.c FROM …` → 2115, `CREATE TABLE link.t (…)` → 2115, `DROP TABLE link.t` →
  2125) is one character after it; at any other token it is the token's own. A
  table named twice without an alias (2132) is placed where an alias would be,
  after the name. tflip places them the same way.
- **[quirk] `AdsDDCreateLink` checks the link's user but not its password.**
  Creating a link logs into the linked dictionary far enough to refuse a missing
  file (7041), an unknown user (5100) and an anonymous link to a dictionary that
  requires logins (5054) — but a wrong password is accepted, and the link then
  fails every statement that uses it (5174 / `Error 5097:  The password given is
  not valid.`). A link that is not global (`ADS_LINK_GLOBAL` clear) is not
  written to the dictionary at all: it exists only for the connection that made
  it (not in `system.links`, and another connection's `name.table` takes `name`
  as a path). tflip does the same.
- **[by-design] Writes through a link are not in the connection's
  transaction.** `INSERT INTO link.t` inside `BEGIN TRANSACTION … ROLLBACK`
  stays written; the linked dictionary's triggers fire and its referential
  integrity and access rights hold for the link's user. tflip does the same.
- **[bug] A table kept open by the cursor cache keeps its old triggers.** A
  closed `SELECT`'s table stays open (`AdsCacheOpenCursors`, 25 by default),
  and a statement that reuses it fires the triggers the table had when it was
  opened: after `SELECT * FROM t`, a `CREATE TRIGGER` on `t` does not fire for
  the next `INSERT`/`UPDATE` (and a dropped one no longer does), on the local
  and on the remote server alike; with `AdsCacheOpenCursors(0)` they fire.
  tflip reads a table's triggers for each statement, so a new trigger fires at
  once (`tools/ace/sql/triggers.txt` sets the cache to 0 to record the
  intended behaviour).
- **[quirk] [eval-server] A commit after `ROLLBACK TO` took back an
  appended record answers 7138**: the rollback to the savepoint releases
  the lock of a record appended through the API, and the later `COMMIT`
  answers 7138 though it has committed (the transaction count is 0). Not
  when a later append reuses the slot, after a whole rollback, for records
  appended by SQL, or on a table open exclusively. The same sequence on a
  DBF table (CDX, VFP, NTX) commits with 0. tflip does the same
  (`fixtures/write/tx_savepoint_lock`, `tx_cursor_after`).
- **[quirk] [eval-server] The handle left on a record a rollback freed
  does not read it again**: it keeps the record image and, in an ADT
  table, its record count (`AdsIsRecordDeleted` 0, the field values kept)
  until it moves, and a whole rollback leaves an ADT handle at BOF with the
  record number past the last slot (`5068` for its field reads). tflip does the same for the handle that wrote
  (`fixtures/write/tx_cursor_after`).
- **[bug] [eval-server] `ROLLBACK` after a `BEGIN TRANSACTION` in a `NO
  TRANSACTION` trigger is an unexpected error.** On the remote server, a
  trigger made `NO TRANSACTION` whose body runs `BEGIN TRANSACTION; INSERT …;
  ROLLBACK WORK;` fails the statement with `[ASA] Unexpected error from
  Advantage Server Agent.  Check Advantage error log for details.`, inside
  5177. tflip rolls the trigger's transaction back and goes on.
- **[quirk] Triggers and transactions.** On the remote server, a record's
  change runs with the triggers it fires in a transaction of their own, two
  levels deep (`::conn.TransactionCount` reads 2 in a trigger, 3 in the
  client's transaction); a failing trigger takes back the record's change and
  what the triggers wrote, but not the records the statement changed before,
  nor the client's transaction, which goes on. `ROLLBACK` in such a trigger is
  out of sequence (5047), even after a `BEGIN TRANSACTION` of the trigger's
  own. With `NO TRANSACTION` the count is the client's and a failing trigger
  keeps its writes — and the change of a failing `AFTER UPDATE` or `AFTER
  DELETE`, but the record a failing `AFTER INSERT` fired on is gone all the
  same. The local server, which has no transactions, keeps every trigger's
  writes and changes, but likewise drops a failing `AFTER INSERT`'s record,
  and accepts `ROLLBACK`. tflip does all of it (`fixtures/sql/triggers.json`;
  the local server's way with real transactions off).
- **[quirk] The 2215 of a `CONFLICT INSERT` trigger misspells "supported".**
  `CREATE TRIGGER … CONFLICT INSERT` is refused with `Invalid create options:
  trigger <name> -- CONFLICT ON INSERT trigger is not suppported.`, before the
  connection is checked for a dictionary. tflip words it the same.
- **[quirk] DBF tables in a dictionary** (`fixtures/sql/ddbf`,
  `fixtures/sql/dbfpk`, `fixtures/dictionary/dbfcorners`). The dictionary's
  character type, not the one a table is opened with, says how a Clipper,
  FoxPro or Visual FoxPro table's text reads; a free Visual FoxPro table
  opened as OEM stays ANSI, but a dictionary's does not. `AdsDDAddTable`
  keeps `ADS_OEM` for a DBF table only, takes a character type of 1 to 118
  for ADT and VFP tables (a Clipper or FoxPro table's must be `ADS_ANSI` or
  `ADS_OEM`, else 5025 with a text about collation languages; 0 or more than
  118 is 5027), and `ADS_DEFAULT` as the table type is 5025 whatever the
  file. A table of another type than its index file is 7015, 7017 or 7108,
  by its own type. A plain `NULL` or `NOT NULL` column of a Clipper or
  FoxPro table is 2215; its `CONSTRAINT NOT NULL` fails with 5042 only
  after the table joined the dictionary, and goes again: the object ids it
  took stay taken and its slots go on the free list. `DROP TABLE` frees a
  table's fields by their names, not by their order in the table. A VFP
  `PRIMARY KEY` is a candidate order of the structural index file
  (`!Deleted()`, 2050 in the dictionary) and refuses a NULL key with 5208;
  a Clipper or FoxPro table's is 5042 ("only supported with ADT and VFP").
  A refused `INSERT` leaves a blank, deleted record. tflip does all of it.
- **[quirk] `CAST(x AS SQL_CHAR)` without a length sizes some types by the
  value.** A `BIGINT` is as wide as the value's digits and one more
  (`CAST(CAST(1 AS SQL_BIGINT) AS SQL_CHAR)` is 2 wide, 123456789012 13),
  where every other number type has a width of its own (`NUMERIC` its
  precision and one more, and one more again with a scale; `DOUBLE` 22;
  `MONEY` 21; `SMALLINT` 6; `TINYINT` 4; `REAL` 13), and `NEWID()` is
  1024 wide. A `NUMERIC` or `MONEY` below one is written without its
  leading zero (`.50`), a `DOUBLE` with it (`tools/ace/sql/castchar.txt`).
- **[quirk] A name refused as taken has used up an object id.**
  `sp_CreateUser`, `sp_CreateGroup` and `sp_AddTableToDatabase` of a name
  already in use fail, but have already taken the next object id and put
  the description in the dictionary's memo, which stays allocated. tflip
  does the same (`fixtures/sql/spdd*`).
- **[bug] `sp_RenameDDObject` of a table stores its index file's path one
  byte long.** With the files renamed too, the index file's path slot is
  one byte longer than the path, and that byte is the one the old path
  had there (or NUL). tflip writes the same bytes
  (`fixtures/sql/spddrename`).
- **[bug] `sp_ModifyTriggerProperty` of `TRIG_CONTAINER_W` (1409) stores
  bytes that are not the value given.** tflip refuses it as not
  implemented.
- **[quirk] `sp_GetLinks` fails with 5054 for every user** on the local
  server, `ADSSYS` included. tflip does the same.
- **[quirk] `sp_SetDDEncryptionType` names its table from the
  dictionary's folder**: the first argument is taken as `<dictionary
  folder>\<name>`, with `.adt` added when it has no extension.
- **[bug] `sp_SetDDEncryptionType` with a `DDPassword` of 21 or more
  characters converts the dictionary and then fails with 2208** ("An input
  value was larger than the input parameter"), the conversion having used
  the password cut to 20 characters (`tools/ace/crypto/FINDINGS.md`). tflip
  fails with 2208 and converts nothing (it does not convert at all).
- **[quirk] A dictionary a process made or opened is converted by
  `sp_SetDDEncryptionType` in that process with any ADSSYS password**, the
  original keeping it open (the same cache as the dictionary password's);
  in a fresh process a wrong one is 7078.
- **[bug] An error in a `FINALLY` after a caught one keeps the old text's
  tail.** In `DECLARE e INTEGER; TRY e = nosuch; CATCH ALL e = 1; FINALLY e =
  1 / 0; END TRY;` the error is "Division by zero nosuch -- Location of error
  in the SQL statement is: 28": the new text is written over the front of the
  caught one ("Column not found: nosuch -- Location …") and the rest shows
  through, its position too; a division by zero in a `FINALLY` with nothing
  caught before is worded right, which is the positive control. Seen under
  Wine only. tflip gives the new error's own text.
- **[bug] An `ALTER COLUMN` that breaks an order's key leaves a dictionary's
  table unopenable.** On a dictionary table with an order `UPPER(b) +
  STR(a, 4)`, `ALTER TABLE t ALTER COLUMN a a CHAR(5)` fails with 3009
  `Unsupported data type in function parameter in index key expression.`
  after deleting `t.adi` and rewriting the table's Field and Key objects;
  `SELECT * FROM t` then fails with 7041 `File not found`, and so does every
  later `ALTER` of it. Positive controls: on a free table the same `ALTER`
  fails with 3009 and rolls back, the table usable; on the dictionary table
  `ALTER COLUMN b b CHAR(9)`, which the key survives, succeeds. tflip does
  not copy it (not in a fixture).
- **[bug] `sp_mgGetConfigMemory90`'s `Total Memory` is 0**, where
  `sp_mgGetConfigMemory` gives 337674 for the same items, their sum (the
  positive control). tflip gives the 0 too (`fixtures/sql/spmg.json`).
- **[bug] A refused `sp_EnableQueryLogging` leaves its table in the
  dictionary.** `sp_EnableQueryLogging('sub\ql', 1, 0, 0, '')` on a
  dictionary connection fails with 5132 (`'sub\ql' is not a table or view in
  the data dictionary`) yet leaves a table `ql` in `system.tables`, its path
  `…\sub\ql.adt` (`fixtures/sql/spddql3.json`). Positive control:
  `sp_EnableQueryLogging('nosuchtable', …)` makes its table and succeeds.
  tflip refuses without making it.
- **[bug] A trigger that empties its `__error` table fails with an internal
  message.** A trigger that inserts a row into `__error` and then deletes it
  makes the statement fail with 5068 (7200 in SQL) `There is no current
  record.  .\cconnect.cpp, 11390`: a source file and line of the engine,
  and no error the trigger raised. Positive controls: without the `DELETE`
  it fails with 5177 and its `TrigError` text; a trigger that never wrote
  `__error` and deletes from it succeeds. tflip gives the same text
  (`fixtures/sql/trigerr.json`).
- **[bug] A `COMMIT` or `ROLLBACK` after a script left a transaction open
  says 5047 but does its work.** After `BEGIN TRANSACTION; BEGIN
  TRANSACTION; INSERT …; COMMIT;` (the count left at 1), the remote
  server's next `COMMIT` or `ROLLBACK` commits or rolls back and returns
  5047 `The transaction command was not in valid sequence.  Not in a
  transaction.`; after `BEGIN; INSERT 81; BEGIN; INSERT 82; COMMIT;`, a
  `ROLLBACK` takes 81 and 82 back and still fails, the count 0 after.
  Positive control: `BEGIN TRANSACTION; COMMIT; BEGIN TRANSACTION;` then
  `COMMIT` returns 0. tflip returns 0 (`fixtures/sql/txscripts.json`, kept
  as known).
- **[bug] Eight bytes cast to a BIGINT are an internal error.**
  `CAST(x'4142434445464748' AS SQL_BIGINT)` fails with 2119 `Internal
  error`, where four bytes give 2236 `not enough binary data`, and the same
  eight bytes cast to a DOUBLE read big-endian. Positive control:
  `CAST(x'41424344' AS SQL_INTEGER)` is 1094861636. tflip reads the eight
  bytes as a big-endian BIGINT (`fixtures/sql/gram_cast.json` records it;
  the replay skips it).
- **[bug] A subquery as an operand of `AND` is an internal error.**
  `(SELECT 1 FROM system.iota) AND 1` and `1 AND (SELECT 0 FROM
  system.iota)` fail with 2119 `Internal error`; positive control:
  `(SELECT 1 FROM system.iota) = 1` works. tflip answers T and F.
- **[bug] `1 - date` is an internal error.** `SELECT 1 - dt FROM ta`, `dt`
  a DATE column, fails with 2119 `Internal error`; positive controls: `dt -
  1`, `1 + dt` and `dt - dt` work (`fixtures/sql/claims.json`).
- **[bug] A SMALLINT shift that gives no value drops its row.** On a table
  with a ShortInt `s` of 1, 15, 16 and 2, `SELECT s << s AS x FROM sh2`
  returns three rows (2, 0, 8) and a record count of 0; the row of `s = 15`
  (15 << 15 is -32768, the SMALLINT that stands for no value) is gone, with
  no error. Positive controls: the same value from a constant
  (`CAST(1 AS SQL_SMALLINT) << 15`) and the INTEGER case (`SELECT a << b`, 1
  << 31) fail with 5179 `The result value exceeded the given buffer size.
  Field: "EXPR"`. tflip fails with 5179 for all of them
  (`fixtures/sql/gram_shift.json` leaves the column's case out).
- **[bug] `UPDATE … FROM` reads a `RIGHT JOIN` as an inner join and a
  `FULL JOIN` as a left one.** `UPDATE r3 SET v = 5 FROM r4 RIGHT JOIN r3 ON
  r3.id = r4.id` leaves the `r3` records with no match in `r4` alone, and
  `WHERE r4.id IS NULL` there matches nothing; `FROM r4 FULL JOIN r3` updates
  the matched records only, where `FROM r3 FULL JOIN r4` and `FROM r3 LEFT
  JOIN r4` update all. No error. Positive control: the same joins in a
  `SELECT` keep the unmatched rows. tflip does the same
  (`fixtures/sql/updfrom.json`).
- **[quirk] `UPDATE … WITH DELETE SET m = NULL` leaks an ADT memo's block.**
  The deleted slot's memo pointer is cleared but its block is not freed (so
  for `SET m = ''`); a memo set to another value has its block freed and the
  value dropped. tflip does the same (`fixtures/sql/updfrom.json`).
- **[bug] A trigger script of exactly 65,535 bytes is accepted and cannot
  be fired.** `CREATE TRIGGER` (and `ALTER TRIGGER`) of a script of 65,535
  bytes succeeds, but every statement that fires it then fails with `7200`
  "AQE Error: State = HY000; NativeError = 5005; [SAP][Advantage SQL
  Engine][ASA] Error 5005: The supplied buffer was insufficient for the
  information returned. The data may have been truncated.
  .\cconnect.cpp, 12678", after its row is already deleted or written.
  Positive control: a script of 65,533 bytes is kept and fires, 65,534
  bytes is refused with `5127` "The property is too long.", 65,536 and more
  with `5021` "Parameter 7 is too big. Container is limited to 64K.", and
  `DROP TRIGGER` of the 65,535-byte one works; so the one size between the
  two refusals is stored but not loadable. It fails every statement of the
  event the trigger is on once firing reaches it (the triggers before it in
  firing order have run), with the effect of any failing trigger on the
  local server: a `BEFORE` trigger leaves the row unwritten, an `AFTER
  INSERT` trigger's row is removed, an `AFTER DELETE` trigger's row stays
  deleted; other events are not affected. The stored script slot has a
  length of 0 and no bytes (65,536 with its NUL, wrapped). tflip does all
  of this (`fixtures/sql/trigbig.json`, `fixtures/sql/trigbig/`,
  `fixtures/sql/limits_procs.json`).
- **[unconfirmed] [eval-server] A sorted `SELECT` right after `ALTER ONLINE
  TABLE` stopped the server.** On a table of 131,072 records, `SELECT *
  FROM big WHERE n < 0 ORDER BY n` just after an `ALTER ONLINE TABLE` left
  the evaluation server accepting no more connections (clients timed out
  with `6602`) until it was restarted; the same query without `ORDER BY`
  worked. Seen once and not reproduced; a run of the same sequence several
  times, and with the `ORDER BY` on an indexed column, would settle it
  (`tools/ace/events/events_online.txt`). Not reproduced in 18 more
  sequences (ascending and descending, after `ALTER ONLINE TABLE` with
  another connection's writes meanwhile, six with the `ORDER BY` on an
  indexed column; `tools/ace/events/events_online_sort.txt`).

- **[bug] A `DB:Public` rights slot that does not check makes listings
  repeat rows.** For an ordinary user, a bad package slot makes `SELECT *
  FROM system.functions` return the first function without end (5000 and
  more rows, a record count of 0), and a bad table slot, when the table is
  not the first the user lists, makes `system.tables` and `system.columns`
  repeat rows (200 and more, count 0). The control: with the slot intact,
  or with the bad table first, the same statements end, or fail with
  `5127` "Inconsistency detected while reading stored property."
  (`tools/ace/sql/ddpublicbad4.txt`, `ddpublicbad5.txt`; slots broken with
  `crates/tflip-storage/examples/pubslot.rs`). tflip fails these with
  5127.
- **[bug] `system.permissions` and `system.effectivepermissions` after a
  bad `DB:Public` slot report 0 records and skip row numbers.** The rows
  that read the slot are lost, the row numbers keep their place in the
  grid, and each later lost row is replaced by a copy of an earlier row;
  the record count is 0 (`fixtures/sql/ddpublicbad3`, `ddpublicbad4`,
  `ddpublicbad5`). The control: the same dictionary with the slot intact
  numbers the rows from 1 and counts them. tflip loses and copies the same
  rows, but numbers and counts what it shows.

- **[quirk] Property id 65535 names no table.** `AdsDDSetTableProperty`
  and `AdsDDSetFieldProperty` with property id 65535 answer `5132` "The
  specified database object name is not valid.  There is no such named
  table in the data dictionary." for a table that exists, whoever the
  user is, where any other unknown id is `5126` before the name is looked
  up (`tools/ace/sql/ddapirights2.txt`). tflip matches it.
- **[quirk] A table-level grant to `DB:Public` stops a deploy as a column
  grant.** `AdsDDDeployDatabase` of a dictionary whose table has rights
  granted to `DB:Public` fails with `5136` "Column level permission is not
  supported by the DB:Public role." although the grant is on the table,
  after it added the other objects; the tables are freed again
  (`fixtures/dictionary/deploy/x_pubt*`). tflip matches it.

- **[by-design] Copies of an encrypted table are written in the clear.**
  `AdsCopyTable` and `SELECT … INTO` of an armed AES table (VFP or ADT)
  write a table laid out for AES but not armed: its records are the
  source's as the handle reads them, in the clear, and a copy made without
  the password holds blank records (`fixtures/aes/rc4tp` v13 to v16).
  `AdsCopyTableContents` into an armed table enciphers as any write does.
  A copy of an encrypted table must be encrypted again by the caller.
  tflip does the same.

- **[bug] `AdsZapTable` by a handle without an AES table's password
  empties the table and then fails.** It fails with 5096 "Cannot reindex
  an encrypted table unless encryption is enabled."; the table keeps its
  index with the old keys (an order of a table of 0 records counts 3
  keys), and the handle is on no record. The control: a handle with the
  password zaps and reindexes without an error
  (`tools/ace/crypto/aesapi/07_zap.txt`, `fixtures/aes/api/07_zap.json`).
  tflip does the same.

---

## Replication objects

- **[quirk] `AdsDDDeleteSubscription` of a renamed subscription leaves its
  queue table**: the queue table keeps the name the subscription was made
  with (`__sa` after `sa` became `sa2`), and the delete looks for
  `__<current name>`, finds nothing and succeeds, leaving the `__sa` table
  and its files. Seen while probing `fixtures/sql/spddrenamerepl` (the
  delete is not in its final script). tflip does the same.

---

## Online backup

All [eval-server]: online backup runs on the remote server only
(`fixtures/backup`, see its README).

- **[bug] `DontOverwrite` does nothing on `sp_RestoreDatabase`.** The help
  says an existing table is kept with a warning; the dictionary restore
  overwrites it and gives no row (`s09_dontoverwrite`). On
  `sp_RestoreFreeTables` it works (code 80 rows). tflip does the same.
- **[quirk] The image's files take the case of the `include=` name.**
  `include=T_CDX` writes `T_CDX.dbf` and `T_CDX.fpt` for the table
  `t_cdx` (`s11_include_exclude`). tflip does the same.
- **[quirk] `include=` does not narrow a free-table backup's mask**; it
  adds the files it names after the mask's. `exclude=` narrows
  (`s40_free_backup`). tflip does the same.
- **[bug] A misspelt message**: `sp_RestoreDatabase`'s
  "Source path must include exisiting dictionary file name." tflip keeps
  the spelling.
- **[bug] [crash] A trigger whose statement inserts into its own `__new`
  table stops the server** (seen while probing, not about backup). Not
  isolated further.

---

## Records, dates and relations

- **[quirk] A VFP field set to NULL keeps its bytes, and a full-text order
  on it keeps the record's words.** `AdsSetNull` on a nullable Character
  field sets the NULL bit and leaves the text, and the order's words come
  from the text, not the flag, so the order is unchanged. tflip does the
  same ([`tools/ace/write/fts.py`](../tools/ace/write/fts.py),
  `fixtures/write/fts_cdx_null`).

- **[bug] A record deleted and recalled in one transaction stays on the
  ADT free list.** After the commit the header has the slot as the free
  list's head (`free_head` 3, `free_count` 2) while record 3 is active;
  `AdsGetRecordCount` says 5 for 6 active records, and the next append takes
  slot 7, not 3. Remote server. tflip takes the slot off the list
  (`fixtures/write/tx_recall`, not replayed).
- **[quirk] Date texts without separators.** In a format with separators the
  engine leaves out spaces and then skips exactly one character after each
  field but the last, whatever it is: under `MM/DD/CCYY`, `01011900` is
  1900-01-11 (the `0` and the `9` skipped), `1108/2017` 2017-11-08,
  `11 8 2017` 1907-11-20 and `11/082017` 0017-11-08, while `11082017` has the
  day 82. tflip reads them the same way (`fixtures/fields/dates.tsv`).
- **[quirk] Date texts in a format without separators.** Under `CCYYMMDD`,
  `YYMMDD` and the like the engine leaves out spaces and reads each field as
  up to its width in digits — the year up to four, `YY` too — skipping
  nothing: `171108` under `YYMMDD` is the year 1711, month 8 and no day
  (`AdsSetDate` fails with 5079, `AE_INVALID_DAY`), `081117 12:34` under
  `DDMMYY` is 1712-11-08, `2017 11 08` under `CCYYMMDD` 2017-11-08, and
  `x20171108`, where no field has a digit, the empty date. tflip reads them
  the same way (`fixtures/fields/dates.tsv`).
- **[quirk] Where a timestamp text's time starts.** The engine ends a
  timestamp text's date at its third separator — a run of non-digits after a
  digit — wherever the date's own fields ended, in any date format: so in
  `1108/2017 13:00` (a digit skipped as the first separator) the date runs to
  the colon and the time is `00`, midnight; in `11/08/201712:00` likewise;
  `1182017 01:02`, short of separators, is all date (0701-11-20, midnight); and
  under `CCYYMMDD`, `20171108 12:34` is midnight too. A TimeStamp value, a seek
  key in an order on a TimeStamp field and `CTOTS`'s argument are all read so.
  tflip reads them the same way (`fixtures/fields/dates.tsv`,
  `fixtures/fields/tstexts.tsv`).
- **[quirk] `CTOTS` and a TimeStamp seek key take a time of a day or more.**
  `AdsSetField` of `11/08/2017 25:00` on a TimeStamp field fails with 5101
  (`AE_INVALID_VALUE`), but `CTOTS('11/08/2017 25:00')` is 2017-11-08 with a
  time of 25 hours (`HOUR()` 25; the day is not carried), and `AdsSeek` of it
  as an `ADS_STRINGKEY` returns 0, seeking a key after that day's last time
  and before the next day's first. `CTOTS` also keeps the time of a text whose
  date is wrong or missing: `CTOTS('13/01/2000 12:34')` is the empty date with
  `HOUR()` 12, where `AdsSetField` and `AdsSeek` fail with 5095
  (`AE_INVALID_DATE`). A time's `AM` takes 12 hours from 12 or more (`13:00
  AM` is 01:00, `25:00 AM` 13:00), `PM` adds 12 to less. tflip does the same
  (`fixtures/fields/tstexts.tsv`).
- **[bug] A failed TimeStamp set can leave the field part-written.**
  `AdsSetField` of `11/08/1712:34` (`MM/DD/CCYY`) fails with 5101 (the time,
  34 hours, is too large) but leaves the field's date 1712-11-08 with its old
  time; `20171108` under `CCYY-MM-DD` and `171108` under `YY.MM.DD` fail with
  5095 yet leave the date 1901-01-19. Other failed sets leave the field as it
  was. tflip leaves it as it was every time (`fixtures/fields/dates.tsv`).
- **[quirk] A past-the-end `AdsGotoRecord` inherits `AE_INVALID_RECORD_NUMBER`
  from a deleted *current* record.** Worked out on **real Windows 10**
  (10.0.19045, native `ace32.dll`): `AdsGotoRecord` to any slot past the last
  record (last + 1, + 2, far beyond) lands the cursor at EOF — `AdsAtBOF` and
  `AdsAtEOF` both true — and returns **`0`**, *unless the current record is a
  deleted record* at the moment of the call, in which case the same past-the-end
  call returns **`5022`** (`AE_INVALID_RECORD_NUMBER`, "…You cannot go to deleted
  records in ADT tables.") while still leaving the cursor at EOF. Whether a
  deleted record exists *elsewhere* in the table makes no difference; only the
  cursor's current record does — positive control: with a deleted record present
  but the cursor on a live record, the past-the-end goto returns `0`. This is
  exactly what the `fixtures/write/goto_deleted` golden records: its `goto 5`
  returns `5022` because the preceding `goto 2` left the cursor on the deleted
  record 2. The misapplied message — the engine complains about deleted records
  while moving *to* EOF — is the quirk. tflip returns `0` and goes to EOF in
  every case (a deliberate difference; see `KNOWN_ISSUES.md`). Probe:
  [`tools/ace/settle/gotopast.txt`](../tools/ace/settle).
- **[quirk] A rolled-back append.** Going to a record a transaction appended and
  then rolled back leaves the engine's cursor at EOF; tflip goes to the deleted
  record as to any other.
- **[quirk] A grandchild whose parent found nothing** gets a `5068` from the
  engine in a relation chain (recorded in the relations fixtures' `known.json`).
- **[by-design] A child with two parents follows whichever parent moved last.**
  Worked out on **real Windows 10** (10.0.19045, native `ace32.dll`): a table can
  be the child of two relations at once — `AdsSetRelation` from parent `p1` (via
  the child's order on `k1`) and from `p2` (via the child's order on `k2`) both
  return `0`. Thereafter, each time a parent is repositioned the child is
  re-sought by *that* parent's relation, so the child sits wherever the
  most-recently-moved parent dictates: over six parent moves it landed on
  `k1 = p1.id` after each `p1` move and on `k2 = p2.id` after each `p2` move.
  There is no fixed precedence between the parents — last mover wins. Probe:
  [`tools/ace/settle/twoparents.txt`](../tools/ace/settle).
- **[bug] `AdsConvertTable` cannot read a Clipper table's General or Picture
  value.** Converting a Clipper (`ADS_NTX`) table whose General (or Picture)
  field holds a value — set with `AdsSetField` — fails with **`7019`**
  (`AE_CORRUPT_MEMO`) even to another Clipper table, the only kind it may go
  to; `AdsGetLastError` meanwhile still reports an older, unrelated message
  ("Error 5169: The table could not be copied by the server so the operation
  was scaled back and copied by the client. The source table was opened
  exclusively."). The record with the value is written with the field blank and the conversion
  stops there. Positive controls: the same table with the field blank converts
  (`0`), and a FoxPro or VFP table's General and Picture values convert with
  their memo blocks (as types 2 and 0). tflip does the same. Probe:
  `fixtures/write/converts_dbf`.
- **[quirk] A conversion that fails part way leaves its file.** A value its
  converted field cannot hold — a Double of `-1e20` or `1e300` in the Numeric
  a Clipper or FoxPro table makes of it — ends `AdsConvertTable` with `5179`
  ("The result value exceeded the given buffer size."), the new table holding
  the records before and the failing one as far as it was set (its later
  fields blank); nothing is removed. tflip does the same
  (`fixtures/write/converts_dbf`, `converts_values`).
- **[quirk] `AdsConvertTable` turns empty values into empty, losing zeros.** A
  field the engine reports empty (`AdsIsEmpty`) is set empty in the new table,
  whatever it held: a VFP field without NULL that holds `0` (an Integer, a
  Double, a Money) becomes NULL in an ADT table and blank in a Clipper one. A
  NULL value, by contrast, depends on where it comes from: from an ADT table a
  Clipper or FoxPro NChar or NMemo stays blank (spaces), from a VFP table it
  takes the zeros those tables read as NULL. An ADT-to-ADT conversion keeps the
  AutoInc values but numbers the RowVersion values afresh (1, 2, ... and the
  header's last value), while a Clipper, FoxPro or VFP table gets the source's
  RowVersion numbers. tflip does the same (`fixtures/write/converts_dbf`,
  `converts_values`).
- **[quirk] `AdsEvalStringExpr` renders a NULL result as a single `0x00` byte** —
  a quirk of that call, not of the expression — so NULL results are excluded from
  the case-folding replay (`crates/tflip-engine/src/table.rs`).
- **[quirk] `AdsSetBinary` in pieces brings back the bytes it cut.** An ADT
  Binary, Image or VarBinary value of 10 bytes (`0102030405060708090a`), set
  with a total of 4, then with a total of 10 and 1 byte at offset 0, reads
  `ff02030405060708090a`: bytes 5 to 10 are the old ones, not zeros. They
  stay across `AdsWriteRecord`, `AdsCancelUpdate` and moving to other
  records and back, and the `.adm` block holds them; they go only on NULL
  (a total of 0 and length 0, `AdsSetNull`, `AdsSetEmpty`) or when another
  record's value is set in pieces. A total longer than any stored pads with
  zeros. No positive control tells whether this is meant
  (`fixtures/write/binary_pieces*`, `tools/ace/write/write.c`). tflip pads
  with zeros (KNOWN_ISSUES).
- **[unconfirmed] A deleted record written again makes a cycle in the free
  list.** In an ADT table open exclusively, after `goto 2; delete; goto 3;
  delete; goto 4; goto 2` (the last fails 5022, leaving the cursor on the
  deleted record), a `delete` and `write` of record 2 push it onto the free
  list a second time: the header's list runs 2, 3, 2 with a count of 3. A
  deleted record that heads the list is not pushed again. Appends after it,
  which would take record 2 twice, are not probed
  (`fixtures/write/deleted_cursor`, table `q5`). tflip does the same.
- **[unconfirmed] [eval-server] `sp_ReindexOnline` signals no
  `__ORFinalStage_<table>`.** On a dictionary table of 131,072 records with
  an index, `sp_ReindexOnline('big')` returned after 120 to 170 ms, and a
  connection that had created the event and waited 8 seconds got nothing,
  with or without a cursor held on the table by another connection; on the
  same server `sp_PackTableOnline` signalled `__OPFinalStage_big` and
  `CREATE ONLINE INDEX` its `__OCIFinalStage_big`. The help may mean an
  event the server signals only when other connections hold the table
  open. Recorded in `fixtures/events/events_online_procs.json`: a count of
  0 after a reindex of a 40,000-record table with a reader writing
  meanwhile.
- **[unconfirmed] [eval-server] A table read by a SQL statement is refused
  a second online pack with 7175.** `sp_PackTableOnline('t')` succeeds once
  on a table that a `SELECT` (or `sp_GetTableSizeInfo`) has read on some
  connection; after that every `sp_PackTableOnline`, `sp_ReindexOnline` and
  `sp_PackAllTablesOnline` that reaches the table answers 7175 "An online
  pack operation is already active on the table.", from every connection,
  whatever the delay. It ends when the connections that read the table
  disconnect, or when the connection that read it reads it again (S P S P
  succeeds; S P P fails). A reindex alone and `INSERT`, `UPDATE` and
  `DELETE` never set it, and neither does a pack of a table no one has
  read; one rule fitting every sequence was not found. Once, a second pack
  and a reindex also got 7175 while a long pack worked. The sequences are
  in the header of `tools/ace/events/events_online_args.txt`. tflip does
  not do it.
- **[bug] `AdsConnect101` with an unknown `Collation` gives a garbled error
  text.** `AdsConnect101("Data Source=<folder>;ServerType=LOCAL;Collation=Bogus;")`
  fails with 7147, but the text `AdsGetLastError` gives is garbage bytes
  (`6h^yopx<6u{x$6xd6u{xL6{...`), not the repository message. Positive
  control: `AdsSetCollation` of `Bogus` on a connection gives "Error 7147:
  The collation name could not be found in the repository.  Bogus"
  (`tools/ace/collation/setcollation.c`). Seen on every run under Wine, not
  checked natively (`tools/ace/config/connect101.c`, the `Collation=` rows
  of `fixtures/config/connect101_values.json`). tflip gives the
  repository's text; the replay compares the code only.
- **[quirk] A few getters name the wrong argument, or none, for a NULL
  pointer** (`fixtures/errors/param_n.json`, `tools/ace/errors/param_n.c`;
  the other calls name their own argument):
  `AdsGetRelKeyPos(index, NULL)` fails with 5021 "Parameter 3", though the
  pointer is the second; `AdsGetAOF(table, buf, NULL)` returns 0 and
  leaves the last error as it was; and `AdsGetLastTableUpdate` neither
  clears the last error nor records its own 5021. tflip does the same.
- **[quirk] `AdsGotoBookmark60` through an order or a connection does
  nothing and returns 0**, where through a table it moves to the record
  (`fixtures/bookmarks/bookmarks.json`). tflip does the same.
- **[quirk] A trigger altered to 65,535 bytes again reads back whole.**
  `CREATE TRIGGER` of a 65,535-byte script (unreadable, 5005), `ALTER
  TRIGGER` to a short script (it fires), then `ALTER TRIGGER` to 65,535
  bytes again: the trigger fires and its script is whole. A short trigger
  altered to 65,535 bytes is the unreadable one. tflip makes both
  unreadable (KNOWN_ISSUES).
- **[quirk] Triggers of one timing and priority fire newest first, but for
  the places of deleted records.** A trigger created into the record of an
  object that was loaded (or deleted) when the connection read the
  dictionary stands at that record's place, so the first triggers made
  after the dictionary's own were dropped fire in the order they were made,
  and later ones before them; the places are given up when a trigger first
  fires, so the same `CREATE`s fire in the opposite order a moment later.
  `ALTER TRIGGER` of a trigger made in the session moves it to the front,
  and `system.triggers` lists by record. tflip does the same
  (`fixtures/sql/trigorder.json`, `trigorder2.json`).
- **[quirk] Triggers and the procedures they call nest 64 levels, a trigger
  counting 2 (the outermost 4) and a procedure 1**; deeper fails with 7093
  in the 5177 and 5154 chain, cut at 562 characters like every error that
  names a failed trigger. tflip does the same
  (`fixtures/sql/trigproc2.json`).
- **[quirk] `CROSS JOIN` is no keyword: `CROSS` is read as the first
  table's alias, so the `JOIN` wants an `ON`.** `SELECT * FROM a CROSS JOIN
  b` fails with 2115 `Expected lexical element not found: ON`, as in
  subselects, views, `SELECT INTO` and scripts; `a CROSS JOIN b ON 1 = 1`
  runs, with `a` named `CROSS` (so `ON a.id = …` is 2122); after an alias
  or an `ON` condition `CROSS` is 2117 `Unexpected token: CROSS`. The cross
  join that works is `FROM a, b`. No `CROSS JOIN` form works, so there is
  no positive control. tflip does the same (`fixtures/sql/gram_cross.json`).
- **[bug] Joining two NCHAR values longer than 32,500 characters together
  fails with 2119 `Internal error`** and no place: `CAST('a' AS
  SQL_WCHAR(20000)) + CAST('b' AS SQL_WCHAR(20000))`. Positive control: the
  same as NVARCHAR caps the result at 32,500, and as CHAR joins up to
  65,529. tflip does the same (`fixtures/sql/claims.json`).
- **[quirk] The text of 2114 and 5012 for a name or number of more than
  1024 characters is cut at its own length for each code** (457 characters
  of the token for 2114, 371 of the name for 5012). tflip gives the codes
  but not the cut (`fixtures/sql/claims.json`).
- **[quirk] The string functions' bounds are uneven**: `SUBSTRING('abc', 4,
  1)` is `''` but `SUBSTRING('abc', 5, 0)` is 2159 for the third argument,
  and `LEFT('abc', 0)` is 2159 where `LEFT('abc', 9)` is the whole text.
  tflip does the same (`fixtures/sql/claims.json`).
- **[bug] An external procedure's `__error` row raises nothing without a
  number other than 0**, though the help ("The structure of AEPs") says to
  leave `ERRORNO` empty to raise its message with the standard AEP error:
  a row of `(NULL, 'text')`, `(0, 'text')` or a message alone, and the
  procedure succeeds. Positive control: `(42, 'no thanks')` fails it with
  5154 `ProcError 42:   no thanks`. tflip does the same
  (`fixtures/sql/aep.json`).
- **[quirk] An external procedure's non-zero return is reported as 5154,
  whatever it returned**: "The function returned with the error code: 5154"
  for a function that returned 7008, and for one that returned the 7200 of
  a statement that failed in it; what it wrote to `__output` is dropped.
  tflip does the same (`fixtures/sql/aep.json`).
- **[bug] `SELECT COUNT(*) FROM t` run by the library of an `AFTER DELETE`
  trigger on `t` still counts the deleted record**: with rows 1, 2 and 3,
  `DELETE FROM t WHERE n = 1` and a library that counts through the handle
  it is given, `COUNT(*)` is 3. Positive control: in the same library
  `COUNT(n) … WHERE n > 0` is 2 and `SELECT n` shows two rows, and a script
  trigger's `COUNT(*)` is 2. tflip counts 2 (`fixtures/sql/aeptrig.json`
  avoids `COUNT(*)`).
- **[quirk] A trigger library's return value and its failing statements
  are ignored**; only an `__error` row fails the change, as the help says
  ("the return code is currently not used"). Unlike an external
  procedure's, a message alone, a NULL number or 0 raise it, as the message
  alone. tflip does the same (`fixtures/sql/aeptrig.json`).
- **[unconfirmed] A trigger function named `Count` is never called**, and
  the change succeeds, where the same function named `Counts` or `Tally` is
  called. Not explained; to probe with a function that logs. tflip calls it
  (the test library names it `Tally`).
- **[quirk] The help names an external procedure's `__error` columns
  `ERRORNO` and `MESSAGE`; they are `errno` and `message`**: `INSERT INTO
  __error (errorno, message) …` fails with 7200. tflip names them as the
  original does (`fixtures/sql/aeptables.json`).
- **[unconfirmed] The local server cannot open an external procedure's
  `__output` through the table API**: every name and option gives 7112,
  with a path to a `tim_N.tmp` in the temp folder, though the help and its
  Delphi sample (`tblOutput.Open`, `Append`, `Post`) open it; SQL's `INSERT
  INTO __output` works. The remote server is not probed. tflip opens it.
- **[quirk] The first external procedure a connection calls finds no
  `#__input` through the table API on the local server** (7112, with the
  path), and later calls find it. tflip finds it every time
  (`fixtures/sql/aeptables.json` begins with a call that opens nothing).
