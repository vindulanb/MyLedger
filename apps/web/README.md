# apps/web

The MyLedger web client: a [Dioxus](https://dioxuslabs.com/) single-page app
compiled to WebAssembly and served as static files by the `crates/server`
backend.

## Not a workspace member

This crate only ever compiles for `wasm32-unknown-unknown`, so it is listed
under `exclude` in the root `Cargo.toml` rather than `members`. Including it
would make `cargo build --workspace` try to build a browser-only renderer for
Windows and fail. It has its own `Cargo.lock` and depends on `crates/core` by
path.

Two consequences worth knowing:

- Build it from this directory, not from the repository root.
- Shared logic worth testing belongs in `crates/core`, where tests run on the
  host. Money formatting lives there for exactly this reason.

## Building

Needs the WASM target and the Dioxus CLI:

```sh
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --locked --version "^0.7"
```

Then, from this directory:

```sh
dx build --release
```

Point the server at the output with `MYLEDGER_WEB_DIR`, then open the server's
address in a browser. The server serves the app at `/` and the API under
`/api/v1`; unknown paths fall back to `index.html` so that reloading a page
like `/transactions` works.

Running the server without a built UI is fine — the API still works, and `/`
explains how to build the interface.

## Why same-origin matters

The session is an `HttpOnly` cookie, which this app can neither read nor write.
It works because the app is served by the very backend it calls, so the browser
attaches the cookie to same-origin requests on its own. Nothing — not the
session, not financial data — is kept in `localStorage`, `sessionStorage` or
IndexedDB. The backend database is canonical.

## Layout

| Path             | What's in it                                             |
| ---------------- | -------------------------------------------------------- |
| `src/main.rs`    | Entry point, routes, and the signed-in/out/unknown gate   |
| `src/api.rs`     | Typed calls against `/api/v1`, using `core`'s wire types  |
| `src/store.rs`   | Shared app state; refetches `/state` after any mutation   |
| `src/login.rs`   | Sign-in screen                                            |
| `src/shell.rs`   | Sidebar, top bar, mobile bottom nav, modal host           |
| `src/pages/`     | One module per screen                                     |
| `assets/`        | Stylesheet                                                |
