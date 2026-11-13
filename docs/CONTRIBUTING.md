# Contributing

How to work on tflip: the rule about sources, the setup, and the loop of
probing the original engine, recording what it does, and making tflip do
the same.

## The clean-room rule

tflip learns how the original engine behaves only from the sources in
[REFERENCES.md](REFERENCES.md): the SDK's headers and sources, the
documentation, and black-box probes, which call the original's API and
record what it answers and what it writes.

- **Never** inspect, disassemble, trace, hook or patch Advantage binaries,
  and never use material derived from doing so, wherever it comes from.
- Format notes from elsewhere are used only for facts tflip's own fixtures
  confirm. Internal function names and addresses of the original are not
  cited.
- What cannot be learnt this way stays undone. The data dictionary's
  encryption is one such case: encrypted properties stay unread.
- The repository holds no Advantage software or documentation. Fixtures
  are files the original engine wrote, with made-up data.

## Setup

- **Rust**, stable: `cargo test` runs every test of every crate on a Linux
  host, the exports of `tflip-ace` included (see
  [PLATFORMS.md](PLATFORMS.md)).
- **cargo-nextest** (`cargo install cargo-nextest --locked`), to run them
  faster: `cargo nextest run --workspace` runs each test in a process of its
  own, so the many tests of `tflip-ace` that hold the process-wide settings
  alone wait for no others (about 35 s on 8 cores, where `cargo test` takes
  over 2 minutes). `.config/nextest.toml` starts the slowest first.
- **The DLLs**: the `i686-pc-windows-gnu` and `x86_64-pc-windows-gnu`
  targets and MinGW; `cargo xtask dist` puts `ace32.dll` and `ace64.dll`
  in `target/dist`.
- **Probing**: Wine, `i686-w64-mingw32-gcc`, and the Advantage Client
  Engine SDK 12.0.0.2 installed in a Wine prefix. The probes take the
  original's `ace32.dll` as a Windows path, such as
  `'C:\Program Files (x86)\Advantage 12.0\acesdk\ace32.dll'`.
- **Collation files**: the tests that need the original's `ansi.chr` and
  `adscollate.adt` look in `TFLIP_COLLATION_DIR`, or in the SDK in the Wine
  prefix `~/src/ace-sdk`, and are skipped without them.

## Probing the original engine

A probe is a small C program that loads an `ace32.dll` by its path, calls
it, and prints what it answers as JSON. The probes in `tools/ace/` each
mostly come with a `run.sh` that builds them with MinGW and runs them under
Wine on fresh copies of their tables (the others are driven by another
probe's `run.sh`, as the READMEs and comments say); the comment at the top
of each says how to call it. [fixtures/README.md](../fixtures/README.md)
says which probe made which fixture.

For a question the existing probes do not cover, a throwaway program in
the same style is quickest:

- run it as `WINEPREFIX=... wine probe.exe <ace32.dll> <data folder>`,
  with the data folder from `winepath -w`;
- turn off stdout's buffering (`setvbuf(stdout, NULL, _IONBF, 0)`): a
  call with a bad handle or a null pointer can crash the original, and the
  output up to there is the answer;
- run `wineserver -k` afterwards, so that no file stays open.

Write down what the probe showed, in the code's comments or in
[KNOWN_ISSUES.md](KNOWN_ISSUES.md) where tflip differs. A probe whose
answer a test should keep becomes a fixture.

Before concluding "the engine cannot do X," prove the object exists and act
on it by the name the engine's own enumeration returns: several entries once
listed as engine bugs in [ADS_BUGS.md](ADS_BUGS.md) turned out to be a
wrong-name-form or missing-precondition mistake in the probe (a trigger needs
its qualified `table::trigger` name, referential integrity needs the parent's
primary key set first). A "the engine fails at X" finding needs that positive
control before it is recorded.

### Validating environment-sensitive findings on real Windows

The probes run under Wine, which does not implement every Win32/Winsock
facility, so a few findings are about the environment rather than the engine:
server discovery (`AdsFindServers`) needs UDP multicast Wine lacks, the DLL's
shutdown faults in Wine's teardown, and the evaluation **server** has been seen
to crash where the client engine does not. Wine cannot settle whether those are
engine defects. A real-Windows run is the oracle that can.

A native Windows **container** cannot run on this Linux host — Windows container
images need a Windows host with Hyper-V. The feasible path is a Windows **VM**
under KVM, e.g. the `dockur/windows` image, which boots a Windows guest with
QEMU/KVM inside a Linux container (it needs `/dev/kvm`, present here, and pulls a
Windows ISO). Install the Advantage Client Engine SDK and, for the server-only
questions, the evaluation server into the guest; share this repository (and the
built probes) into it over a mounted folder or SMB; run the same
`tools/ace/*/run.sh`-style probes there; and diff the Windows answers against the
Wine ones.

This stays black-box (API calls only; no disassembly), so it does not touch the
clean-room rule. What it settles: the `[artifact]` entries (multicast discovery,
the teardown fault, a wild-handle dereference) and the `[unconfirmed]`
eval-server crashes, and it promotes the "checked only on Wine" findings (the
integer-`AVG` bug, the large-double formatting) to "confirmed on Windows." It is
a one-time oracle investment — a Windows ISO and an SDK install — not a
prerequisite for building or testing tflip, and it could later serve as a second
CI oracle that flags any Wine-only divergence automatically.

## Fixtures and their tests

Most of tflip's tests replay a fixture: they make tflip do what a probe
did and expect what the original answered, or the files it wrote, byte
for byte.

- **Reading**: the tables in `fixtures/<type>/` and their JSON dumps, made
  from the SQL scripts in `tools/ace/fixtures/`.
- **SQL, expressions, filters, field getters, error texts**: the JSON files
  `tools/ace/sql/`, `expr/`, `fields/` and `errors/` record.
- **Writing**: the scenarios of `fixtures/write/`. To add one:
  1. write its `setup.txt` and `ops.txt` for `tools/ace/write/write.c`
     (the commands are listed at the top of `write.c`);
  2. record it with the original:
     `tools/ace/write/run.sh <work folder> <ace32.dll> fixtures/write/<name>`,
     and copy `before/`, `after/`, `result.json` (and `volatile.json`, if
     it is not empty) into the scenario's folder;
  3. add a test that calls `check("<name>")` in
     `crates/tflip-ace/src/writes.rs`, and its name to the `remote_twins!`
     list there, so that it also runs through `tflipd`;
  4. describe it in the table of [fixtures/README.md](../fixtures/README.md).

  While working on a scenario, `TFLIP_WRITE_SCENARIO=<work folder> cargo
  test -p tflip-ace scenario_from_env -- --ignored` replays the one in the
  work folder without copying it first.

## Checks before a commit

- `cargo nextest run --workspace` (or `cargo test --workspace`), `cargo clippy --workspace --all-targets` and
  `cargo doc --workspace --no-deps`, without warnings.
- **The wire format**: a change to the engine's API or to the types it
  carries fails `tflip-protocol`'s `wire_format` test. Raise
  `PROTOCOL_VERSION` and renew the copies with
  `TFLIP_BLESS=1 cargo test -p tflip-protocol --test wire_format`.
- **The DLLs**: `cargo xtask dist` builds both. A new export must have the
  exact parameter list of `ace.h`, or the 32-bit stdcall decoration (and
  the client's stack) goes wrong. Running probes against tflip's
  `ace32.dll` under Wine is done by hand, where a change touches the FFI
  layer.
- **The docs**: [KNOWN_ISSUES.md](KNOWN_ISSUES.md) for a difference found
  or fixed, [TODO.md](TODO.md) for what is planned,
  [fixtures/README.md](../fixtures/README.md) for a new fixture, and the
  unreleased section of [CHANGELOG.md](../CHANGELOG.md).

## Diagnostics

`TFLIP_DEBUG` turns the DLL's log channels on and off (`fixme` for stubs,
`remote`, `+sql` for the statements clients run, and `+err` for every call
that fails), and
`TFLIP_LOG_FILE` sends them to a file; see `tflip-ace`'s `log.rs`.
