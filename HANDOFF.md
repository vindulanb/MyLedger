# MyLedger — handoff

Written at the end of Master Prompt C. The backend (Prompt A) and the web app
(Prompt C) are built, tested and verified. The Android app (Prompt B) has not
been started.

---

## Building and running it locally

Three things to build, in this order. Only the first two are needed to have a
working API; the third is the browser interface.

```sh
# 1. The backend, and its tests.
cargo build --workspace
cargo test  --workspace

# 2. The one-time user. The app is single-user and refuses to start without one.
cargo run -p server -- setup --username <name> --password <password>

# 3. The web interface. Needs the WASM target and the Dioxus CLI, once:
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --locked --version "^0.7"

cd apps/web && dx build --release && cd ../..
```

Then start it:

```sh
cargo run -p server
```

It listens on `127.0.0.1:8787` by default and finds the built interface at
`apps/web/target/dx/web/release/web/public` without being told. Open
<http://127.0.0.1:8787> and sign in.

Running the server with no interface built is fine — the API still works, and
`/` prints the command to build the UI rather than a bare 404.

### Settings

| Variable                 | Default                   | What it does                                  |
| ------------------------ | ------------------------- | --------------------------------------------- |
| `MYLEDGER_DB_PATH`       | `data/myledger.sqlite`    | The database file                             |
| `MYLEDGER_DATA_DIR`      | `data/documents`          | Where uploaded files are stored               |
| `MYLEDGER_WEB_DIR`       | the `dx build` output     | Where to find the built interface              |
| `MYLEDGER_BIND`          | `127.0.0.1:8787`          | Listen address                                 |
| `MYLEDGER_COOKIE_SECURE` | off                       | Mark session cookies `Secure` (see below)      |
| `MYLEDGER_LOG`           | `info`                    | Log filter                                     |

### Reaching it from another device on your home network

Bind to the network rather than to loopback, and use the machine's LAN address:

```sh
MYLEDGER_BIND=0.0.0.0:8787 cargo run -p server
```

Your phone then reaches it at `http://<this-machine's-IP>:8787`. Nothing else
has to change: the session cookie is same-origin, and the pairing QR code
carries whatever address the browser used, so a phone scanning it gets the
right one automatically.

### Reaching it from outside your home network

This is a bigger step than it looks, and the honest summary is: it is a
networking and TLS job, not a code change.

What it needs, roughly:

1. **A name and a certificate.** A domain (or a dynamic-DNS name) pointing at
   your home connection, and a certificate for it — Let's Encrypt via
   `caddy`, `nginx` or `rustls-acme` are all reasonable.
2. **A reverse proxy terminating TLS** in front of the server, forwarding to
   `127.0.0.1:8787`. The server speaks plain HTTP by design; it does not
   terminate TLS itself.
3. **`MYLEDGER_COOKIE_SECURE=1`.** Once you are on HTTPS, turn it on. It is
   off by default because a `Secure` cookie over plain HTTP is never sent at
   all, which makes login appear to succeed and then silently fail — a very
   confusing first run on a home network.
4. **A way in through your router**, either a forwarded port or a tunnel
   (Tailscale and Cloudflare Tunnel both avoid opening a port).

Two things worth saying plainly before you do any of that. The password is the
only thing between the internet and your entire financial history, and there
is no rate limiting on the login endpoint beyond the constant-time comparison
— so pick a long password, and prefer a VPN or a private tunnel over a
publicly forwarded port. And the exposure is permanent in a way a LAN is not:
a forwarded port is reachable by anything that scans it.

---

## It starts empty

Confirmed, and tested. On a fresh database:

- No transactions, documents, budgets, goals, repeating items, rules, SMS
  parsing rules or tags.
- Net worth reads **Not set** on the dashboard — not `$0.00`, and not a figure
  guessed from income minus spending.
- The period selector is on **All time**.
- The only seeded rows are category and account *names* — a starting list you
  are meant to edit, carrying no amounts. Deleting them all and restarting
  puts them back; renaming them is respected and never undone.

No sample or demo financial data is inserted at any point, by any code path.

---

## How duplicate detection works

Every transaction gets a fingerprint:

```
date | merchant | amount_cents | account_name
```

Merchant and account name are trimmed and lowercased, so the same purchase
fingerprints identically however a bank export happened to capitalise or pad
it. Nothing else is normalised: two different dates, amounts or accounts are
two different transactions. The account is matched by *name* rather than ID,
because an import often knows only what the bank called it.

A row whose fingerprint is already in the ledger is counted as a duplicate and
skipped — not rejected, not merged, and not reported as an error. Re-importing
the same statement is therefore safe and idempotent: the second run inserts
nothing and tells you how many rows it recognised.

The practical limit: two genuinely separate purchases at the same shop, for the
same amount, on the same day, from the same account are indistinguishable from
one purchase imported twice. The second is treated as a duplicate. Add it by
hand if that happens — the manual form reports the clash rather than silently
dropping it.

Categorisation rules run *after* duplicate detection, so a rule can never
change what counts as the same purchase.

---

## How recurring and subscription detection works

It reads the last three years of transactions and proposes patterns. It never
writes: a suggestion becomes a bill or a subscription only when you accept it.

Per merchant:

1. The many spellings a bank uses for one merchant are collapsed into one
   grouping key, so `TESCO STORES 2831` and `Tesco Stores` count as the same
   shop.
2. The gaps between charges are reduced to one dominant interval and named —
   weekly, fortnightly, monthly, quarterly, yearly. The statistic is the
   *median* gap, so one unusual month does not move it.
3. The price has to be steady. "Steady" depends on what the charge looks like:
   a named subscription may drift by 20%, a bill with a word like *rent* or
   *insurance* in it by 35% (a utility bill genuinely moves with the season),
   and a merchant matching no hint list at all by only 3% — and it must have
   happened at least three times. That last bar is deliberately strict so a
   weekly grocery run is never called a subscription.
4. Variation is measured as `(max − min) ÷ mean`.
5. A pattern is confident when it has three or more occurrences, under 12%
   price variation and under 5 days of schedule jitter. Anything else is
   offered as *likely* and labelled as such.
6. A pattern goes quiet after three missed periods — one expected, two overdue.
   A monthly charge unseen for three months has almost certainly been
   cancelled, and going on suggesting it is just noise.

Dismissing a suggestion hides that merchant for good. It is reversible:
**Settings → Automatic detection → Restore ignored suggestions**.

Two deviations from the letter of the spec, both deliberate: the merchant hint
lists match whole words rather than substrings (so "studio" does not fire on
"studious"), and a confirmed item never also appears as a suggestion, which is
what keeps a combined monthly total from double-counting.

---

## What document extraction can and cannot do

Be clear-eyed about this one. It is **pure Rust with no OCR**, by your choice.

**It can:**

- Read the text layer out of a PDF — a bank statement, an emailed invoice, a
  receipt that was generated rather than photographed.
- Read plain text and CSV-ish text files.
- Run your extraction rules (regular expressions with capture groups for
  amount, merchant and date) over that text, and create a transaction when
  exactly one rule matches unambiguously.
- Cap itself at 20,000 characters of extracted text, and survive a malformed
  PDF without taking the server down — the extractor runs inside a
  `catch_unwind`.

**It cannot:**

- Read a photograph or a scan. There is no OCR in this build. A phone snap of
  a till receipt lands at `status = review` with a sentence saying exactly
  that, and you add the details by hand. It does not guess, and it does not
  pretend to have failed for some other reason.
- Read a spreadsheet. `.xlsx` and friends are refused with a pointer at the
  CSV import, which is the right tool for that data.
- Decide between two readings. If two rules match and disagree, the document
  stays at `review` and shows you both so you can choose. If a captured date
  is ambiguous (`03/04/2026` — which is the month?), the whole candidate is
  discarded rather than resolved by a coin toss.
- Remember what it read *as fields*. Only the extracted text is stored, never
  the amounts and merchants pulled out of it, because rules change — so
  re-reading a document always reflects the rules as they are now rather than
  as they were.

If you later want scanned receipts read automatically, that is an OCR
dependency — `tesseract` via bindings, or a pure-Rust engine — and a decision
to revisit, not something this build is quietly missing.

---

## Verification

Every page was exercised in a real browser (headless Edge, driven over the
Chrome DevTools Protocol with genuine input events) at three widths:
**1440×900**, **768×1024** and **390×844**. At each width, on each of the nine
pages, the checks were: no horizontal page overflow, nothing extending past the
viewport, no clipped headings or figures, and both global dialogs fitting the
window and scrolling inside themselves.

Also exercised: every sidebar link and every bottom-nav tab (including that
each tab is genuinely tappable rather than merely present), every period chip,
the search and category filters, a manual entry through a refresh, inline
category and tag edits through a refresh, a re-imported CSV for duplicate
detection, a document uploaded and deleted, net worth set and unset, one of
every other kind of record created and deleted, and the wipe gate with both a
wrong and a right confirmation.

Totals at the end of Prompt C: **528 Rust tests** (host-run, `cargo test
--workspace`), clippy clean on both the workspace and the `wasm32` target, and
a per-step browser script for each of steps 1–10.

The app is left in its empty start state with the period on All time. Every
temporary artefact those checks created was removed.

### Nothing here touches your phone's SMS

Checked by grep over the whole of `apps/web`, and by the browser scripts: the
web app contains no telephony API, requests no permission, and never reads a
message. The only SMS-related thing in it is the *rule authoring* screen —
text fields holding regular expressions. Those rules sync down to the Android
app, which is the only thing that will ever read an inbox. The reason they are
written here is that a regular expression is miserable to type with a thumb.

---

## Pairing a phone, when you are ready for Prompt B

**Settings → Mobile pairing → Generate pairing token.**

Give the device a name if you like, and you get a token and a QR code. The QR
carries both the token and the address the browser reached this server on, so
scanning it is the whole of setup; typing them in by hand works just as well.

The token is shown **once**. Only its hash is stored, so there is no way back
to it — leaving the screen or pressing *Done* loses it, and nothing in the API
will return it again. If you lose one before the phone is set up, generate
another; the stale one is harmless.

Two properties worth knowing:

- A phone holding a device token **cannot mint more of them**. Pairing requires
  a signed-in browser. One stolen phone costs you one device, not the ledger.
- Erasing all data does **not** unpair anything, and does not sign you out.
  There is currently no way to revoke a single device token short of editing
  the database — worth adding when Prompt B exists and you can test it.

---

## Known limitations, carried forward

Not bugs, but things to know:

1. `PUT /preferences` validates the whole request before applying any of it,
   but applies group by group against the pool rather than in one transaction.
   A crash mid-apply could leave a partial update. The wipe *is* atomic.
2. `GET /state` returns at most 5,000 transactions, with a
   `transactions_truncated` flag. Tag usage counts are computed in SQL
   precisely so they are not affected by that cap.
3. sqlx queries are runtime-checked rather than `query!` macros, deliberately:
   building needs neither `sqlx-cli` nor a `DATABASE_URL`.
4. A mutation started from a row or dialog can still be cancelled by
   navigating away mid-save. The period selector is fixed structurally — the
   app shell owns that task — but the pattern is not applied everywhere.
5. `apps/web/target/dx/` keeps every past build's hashed assets. Harmless, and
   `cargo clean` in `apps/web` clears it.
6. A period reaching the present has no end bound rather than ending today, so
   a transaction dated later this month is still visible. That is a deliberate
   deviation from a literal reading of the spec.
7. There is no backup. Taking a copy of the SQLite file and the documents
   directory is the whole of it, and the wipe has no undo.
