# STUDIO PLAN — a replacement for Advantage Data Architect

tflip-studio replaces Advantage Data Architect (`arc32.exe`): a web-based
admin tool for tflip that works on local tables and dictionaries and against
`tflipd` servers. Managing installations of the original engine is not a goal.

Companion: [`docs/TODO.md`](docs/TODO.md) (the engine's roadmap). When a step here lands,
it moves to [`CHANGELOG.md`](CHANGELOG.md) and the entry here is pruned.

## Architecture

```
browser (Vue 3 + TypeScript)
   │  HTTP/JSON + WebSocket
   ▼
tflip-studio (Rust, axum) ── session layer ──> tflip_protocol::Api
                                                ├─ local engine (in-process)
                                                └─ tflipd client (remote)
```

## Decisions

### Frontend

- **Vue 3** with `<script setup lang="ts">`, built with **Vite**.
- **No component library.** A web-style UI, not a copy of ARC: pages with
  URLs (`vue-router`) instead of a tree, actions as buttons on each page
  instead of menus, native `<dialog>` and `popover`, own CSS. Headless
  **Reka UI** is the fallback for a widget whose behaviour proves hard.
  PrimeVue left MIT with version 5, and a library's look is hard to bend.
- **Grid and SQL editor: decided when their features come.** Start with an
  own read-only virtual table over the server cursor and a plain
  `<textarea>`; candidates beyond that are TanStack Table, AG Grid Community
  and CodeMirror 6.

### Backend

- A new workspace crate, **`tflip-studio`**, serving HTTP with **axum**.
- It builds on **`tflip_protocol::Api`**, not on `tflip-engine` directly, so
  both modes share one code path:
  - **local, in-process:** open a folder or a dictionary on disk, with no
    server running;
  - **remote:** connect to a `tflipd` as its client.
- A thin internal **session layer** turns `Api`'s handles into sessions, open
  tables and cursors, so the HTTP handlers never touch raw handles.

### Session layer

- **One backend per connection.** Each Connect makes its own: a fresh engine
  `Session`, or a `Client` to a `tflipd`. Disconnecting drops it, and its
  `Drop` closes everything it had open, so there is no handle bookkeeping.
  `tflipd` already keeps one session per client, so the engine copes with
  several sessions on the same files in one process.
- **The browser sees opaque IDs, never handles.** A Connect returns an ID
  from a counter (`/api/connections/1/tables`); cursors get IDs under their
  connection (`/api/connections/1/cursors/7/rows`). Handles mean something
  only inside their own backend, and two backends hand out the same numbers.
- **One async bridge.** `Api` blocks on disk and network, so every call goes
  through one helper that runs it with `spawn_blocking`, off tokio's worker
  threads, and turns the engine's error into an `ApiError`.
- **Connections live until Disconnect or exit,** with no idle timeout: one
  local user, and a timeout would close a table someone is looking at. An
  unknown ID is the engine's `InvalidHandle`, an `ApiError` like any other,
  and the frontend returns to Connect.
- **Local first.** The remote Connect needs a decision on TLS trust (which
  certificates the studio accepts), so it follows the local one.

### Connection repository

- **The studio remembers its connections** in an ADT table of its own,
  `connections.adt`, which it reads and writes with SQL through the engine.
  It lives in the **roaming** application data (`%APPDATA%\tflip-studio`,
  `~/.local/share/tflip-studio`): ADS data usually sits on mapped drives and
  UNC paths, the same on every machine of a domain.
- **Every successful Connect is remembered,** the last used first; a mistyped
  path never lands there, and Forget removes the rest.
- **Entries have GUIDs** (`NEWID()`), so whoever adds one can make its ID.
- **The last used is reopened once per start,** the first time the Connect
  page loads; a Disconnect later leads to the list, not to a reconnect.
- **Passwords are stored encrypted, under a master password** (as pgAdmin
  does): Argon2id makes the key, each password is sealed with it, and the
  table keeps only a salt and a check value. The key stays in memory until
  the studio exits, asked for the first time a stored password is needed.
  Until then, an entry with a user fills the Connect form.

### No stable public Rust API

- The backend lives in the workspace and may use the internal crates freely;
  nothing it uses has to be semver-stable.
- **ACE (`ace.h`) stays the only stable public interface.**
- If people later want to script admin tasks, the HTTP API is the candidate
  for a public, versioned API. That decision waits until someone asks.

### The Rust ↔ TypeScript boundary

- **HTTP/JSON** for commands.
- **WebSocket** for what streams or holds state: cursors, long SQL runs,
  progress of pack and reindex.
- **TypeScript types generated from the Rust types** with **`ts-rs`**, so a
  backend change the frontend has not followed fails to compile. `cargo test`
  writes them to `web/src/bindings/`, which is committed; specta's wider reach
  (typed RPC, Tauri) buys nothing for plain axum routes.
- **Cursors live on the server.** The grid asks for a page of rows of a cursor
  ("rows N..N+100 of cursor X") as it scrolls.
- **API errors are one generated type** carrying the ADS error code and
  message, so every view shows errors the same way, with the codes users know
  from ARC and ACE.

### Packaging and versions

- The built frontend is **embedded in the binary** (`rust-embed`); a
  `cargo xtask` step runs the Vite build. One file to ship, and frontend and
  backend cannot drift apart.
- During development, the Vite dev server runs separately and proxies API
  calls to the backend.
- tflip-studio and `tflipd` are **released together** from the workspace; the
  protocol's `PROTOCOL_VERSION` check rejects a mismatched pair.

### Security

Even on localhost, any web page the user visits could talk to the server, so:

- the server **binds 127.0.0.1**;
- it **checks the `Host` header** of every request, against DNS rebinding:
  only `localhost` and loopback addresses on the studio's port pass, and
  anything else gets a bare 403;
- every **API request** (`/api/*`) carries a **random session token**, 32
  bytes from the OS, made anew at each start. The binary prints (later:
  opens) `http://127.0.0.1:6263/#token=…`; in the fragment, the token never
  reaches the server, a log or a `Referer`. The frontend keeps it in
  `sessionStorage` and sends it as `Authorization: Bearer`. A missing or wrong
  token gets an empty 401, which the frontend tells apart from every ADS
  error. The static files need no token, as they hold no secrets.

**A header, not a cookie:** cookies ignore ports, so every other web server on
localhost would receive the token, and `SameSite` counts all of localhost as
one site. The browser never adds a header on its own. Where a header cannot
go:

- **WebSockets** carry the token in the URL's query string, and the upgrade
  request's `Origin` is checked;
- **downloads** go through `fetch`, or a short-lived link the server hands
  out.

`tflipd` does not serve the UI. A database server that also serves a web UI
exposes more to attack, so the two stay separate processes; remote admin means
running the studio locally against a remote `tflipd`.

## Features, in order

1. **Connect:** a local folder or dictionary, or a `tflipd` URL with login.
2. **Table browser and editor:** a virtualized grid over a server-side cursor.
3. **SQL utility,** with `SHOW PLAN`.
4. **Table and index designer:** create, restructure.
5. **Dictionary admin:** users, groups, rights, RI, triggers, views,
   procedures, links, replication.
6. **Utilities:** pack, reindex, zap, copy, export, encryption.
7. **Server management:** connections, locks.
8. **FTS indexes.**

## Future ideas

### A configurable listen address

The address is a constant in the code for now. Later it should come from, in
order of precedence:

1. a **command-line flag**, `--listen ADDRESS`, as `tflipd` has;
2. a **config file** in the user's config directory (`~/.config/tflip/` or
   `%APPDATA%`), which later settings can share;
3. the **built-in default**, `127.0.0.1:6263`, when neither is given.

- Only **loopback addresses** are accepted, and the studio refuses to start on
  any other: it serves plain HTTP, and the token would be its only
  protection. This keeps the Security section's "binds 127.0.0.1" true in
  spirit, so that line changes with it.
- The `Host` check takes the port from the address in use, not from a
  constant. If port `0` is allowed, both the check and the printed URL take
  the address from the listener.
- **The frontend follows in lockstep.** The embedded build calls relative
  URLs and needs nothing, but the Vite dev proxy hard-codes the backend
  address. It should read the same setting, so changing the port in one place
  changes it for development too.
