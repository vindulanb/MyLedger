# MyLedger — Master Prompt C: Web App

Run **Master Prompt A (Foundation)** first — this file assumes the workspace, `core`/`db`/`server` crates, and the backend API already exist and work. This file only builds `apps/web`. (Master Prompt B, the Android app, can be run before or after this one — they don't depend on each other, only on Foundation.)

## How to use this file

Each `## Step N` section is one self-contained prompt. Copy everything under **"Prompt:"** (down to, but not including, "Quick check:") and paste it as your next message to Claude Code. Let it finish, run the **Quick check** yourself, then move to the next step.

Progress checklist:

- [ ] Step 1 — App scaffold, login & shell
- [ ] Step 2 — Dashboard page
- [ ] Step 3 — Transactions page & Add entry
- [ ] Step 4 — CSV statement import
- [ ] Step 5 — Documents page & extraction pipeline
- [ ] Step 6 — Recurring & Subscriptions pages
- [ ] Step 7 — Budgets & Goals pages
- [ ] Step 8 — Rules & Tags page + SMS rule authoring
- [ ] Step 9 — Settings page
- [ ] Step 10 — Responsive polish, full verification & completion report

## Ground rules that apply to every step in this file

1. Write the UI in [Dioxus](https://dioxuslabs.com/) targeting the `web` (WASM) renderer, inside `apps/web`, served by the `crates/server` Axum backend built in Master Prompt A.
2. **This app must never request or read SMS/telephony data in any way.** That capability belongs only to the Android app in Master Prompt B. If you find yourself about to add anything SMS-related here beyond viewing/editing the *stored rule definitions* (text config, not device access), stop — that's out of scope for this file.
3. Money is `i64` cents everywhere; format to currency only at render time.
4. Never use `localStorage`/`sessionStorage`/IndexedDB as the source of truth for financial data — the backend database is canonical.
5. Every step is idempotent — extend what already exists rather than duplicating it.
6. No sample/demo financial data, ever, at any step.

---

## Step 1: App scaffold, login & shell

**Prompt:**

Inside the existing MyLedger workspace (backend already built under `crates/`), set up `apps/web` as a Dioxus web (WASM) application, built with `dx build --platform web` (or `trunk`), served as static assets by the `crates/server` Axum backend from Master Prompt A (add a static-file-serving route/fallback in `crates/server` now if it isn't already there).

Build a login screen using the backend's `POST /api/v1/auth/login`, storing the resulting session via a normal browser cookie (not localStorage). Once logged in, build the real app shell: a left sidebar (~240px) with navigation, a sticky top bar (~72px) with global **Import** and **Add entry** actions, and placeholders for every page this app will eventually have, in this order: Dashboard, Transactions, Recurring, Subscriptions, Budgets, Goals, Documents, Rules & Tags, Settings. On mobile-width browsers, collapse to a compact top bar plus a horizontally scrollable bottom nav reaching all tabs.

Wire up a data layer that fetches `GET /api/v1/state` on load and refetches after any mutating call, so every page always reflects the backend's current state (no client-side caching that can drift from the database).

**Quick check:**

- Logging in with valid credentials succeeds and shows the app shell; an invalid password is rejected with a clear message.
- Refreshing the page keeps you logged in (session persists via cookie, not client storage).
- Confirm no `localStorage`/`sessionStorage` calls exist anywhere in the code for financial data.

---

## Step 2: Dashboard page

**Prompt:**

Build the real Dashboard screen in `apps/web`, replacing its placeholder:

- A period selector: All time / This month / Last month / Last 3 months / Last 6 months / This year. Default and first-ever value is **All time**. Persist via `PUT /api/v1/preferences` (`selected_period`); if saving fails, revert to the previous selection and show an error; on load, always fetch the saved value before rendering date-dependent totals (avoid a flash of the wrong period).
- Four summary cards, reflowing 4→2→1 columns responsively: **Net Worth** ("Not set" + link to Settings until configured, otherwise assets − liabilities), **Income** (sum of income transactions in period), **Spending** (sum of expense transactions in period), **Savings rate** (`0%` when income is zero, no division errors).
- A responsive cash-flow chart (up to 7 monthly points, income vs. expense) that resizes without clipping labels, and a spending-by-category donut with an accessible legend, values, and percentages — both with proper empty states when there's no data yet.
- The 5 most recent transactions in the selected period.
- A factual insight line only (e.g. count of "Needs review" transactions) — never invented advice or fake trend arrows unless both a current and prior real period exist to compare.
- A "Coming up" section for confirmed recurring/subscription items due soon (empty state + link to Recurring for now, since those pages come in a later step).

**Quick check:**

- Fresh backend → Dashboard shows correct empty states, Net Worth says "Not set".
- Add a couple of test transactions directly via the API, refresh, confirm the cards/charts update correctly.
- Resize the browser to ~1440px, ~768px, and ~390px and confirm the cards reflow without clipping.

---

## Step 3: Transactions page & Add entry

**Prompt:**

Build the real Transactions screen in `apps/web`, replacing its placeholder:

- Search (merchant/category/tag), account filter, category filter, and the same shared period selector as the Dashboard (changing it on either page updates the other, since both read the same persisted `selected_period`).
- Table/list with date, merchant, category, account, tags, amount (income `+` in a positive color, expenses `−`), a receipt indicator, and a source badge distinguishing `manual`/`csv`/`document`/`sms` (note: `sms`-sourced rows only ever appear here after the Android app syncs them — this web app never creates them).
- **Inline category editing**: click the category cell → dropdown of managed categories → saves immediately via `PATCH /api/v1/transactions/:id`, no full reload.
- **Inline tag editing**: removable pills + a `+` opening a tag-only modal (never asks for a category).

Build the global **Add entry** modal: Expense/Income segmented control, amount starting blank, merchant starting blank, date defaulting to today (editable), category, account, tags, an "I have a receipt to attach" checkbox revealing a file picker. Validate before saving, persist via `POST /api/v1/transactions`, close on success, refresh affected data.

**Quick check:**

- Add a temporary transaction, confirm correct formatting, then delete it.
- Edit a category and a tag inline, refresh the page, confirm both persisted.
- Confirm the period selector stays in sync with the Dashboard.

---

## Step 4: CSV statement import

**Prompt:**

Add CSV bank/card statement import to `apps/web`, reachable from the global **Import** action:

1. Backend: add `POST /api/v1/import/csv` to `crates/server` if it doesn't already exist — accepts an uploaded CSV, parses rows, recognizes common headers for date/description/merchant/amount/debit/credit/category/account, normalizes dates to `YYYY-MM-DD`, stores amounts as positive `amount_cents`, maps debit/negative rows to `expense` and credit/positive rows to `income` (honoring the statement's actual sign convention, not assuming one), preserves a recognizable category or falls back to `Needs review`, sets `source = 'csv'`/`receipt = false`, and runs every row through the same fingerprint dedupe as every other insert path. Returns inserted/duplicate/skipped/needs-review counts. Never creates placeholder rows for unparseable lines.
2. Frontend: a CSV picker with a column-preview step; if header mapping is ambiguous, show an explicit mapping UI rather than guessing silently; after import, show the result counts clearly.

**Quick check:**

- Import a small controlled test CSV (a handful of rows you write yourself, mixing debits and credits) and confirm correct parsing and sign handling.
- Re-import the exact same file and confirm every row is now reported as a duplicate, not inserted again.
- Delete the temporary imported test rows afterward.

---

## Step 5: Documents page & extraction pipeline

**Prompt:**

Build the real Documents screen in `apps/web`, replacing its placeholder, using the `POST /api/v1/documents` storage endpoint from Master Prompt A: an **Upload documents** card (receipts, invoices, statements, images, PDFs, spreadsheets, ≤20MB each) and a secure vault list (filename, type, size, source, status, imported date), clearly distinguishing `stored`/`queued`/`review`. Empty state: "No documents yet. Upload a file to get started." Never display raw storage paths.

Then add a best-effort extraction pipeline, and be honest in your completion report about exactly what it can and can't do:

1. If a Rust OCR binding is available in this environment (e.g. a `tesseract`-based crate), use it to pull raw text out of uploaded images/PDFs. If it isn't available or reasonably installable, skip OCR and say so plainly rather than faking it.
2. Reuse the same rule-based extraction *concept* as the mobile app's SMS parser: let the user define simple "document extraction rules" (a text pattern → merchant/date/amount/type mapping) in a small settings section on this page, and run OCR'd text through them.
3. Only create a transaction from an extracted document when the match is unambiguous; otherwise leave the document at `status = 'review'` and let the user manually fill in and link a transaction — never invent a value you're not confident in.

**Quick check:**

- Upload a small test file, confirm it's stored and listed with correct status, then delete it.
- If OCR is wired up, test it against one real receipt image and confirm it either extracts correctly or honestly falls back to `review` rather than guessing.
- Confirm the completion note for this step accurately describes what extraction actually works today.

---

## Step 6: Recurring & Subscriptions pages

**Prompt:**

Build the real Recurring and Subscriptions screens in `apps/web`, replacing placeholders, using the backend's `GET /api/v1/recurring/suggestions` and confirmed `recurring_items` from Master Prompt A, filtered by `kind = 'recurring'` vs `kind = 'subscription'`:

- An "Active detection" status banner.
- A suggestions panel with **Keep**/**Ignore** actions showing merchant/service, category, cadence, occurrence count, confidence, average charge, estimated monthly equivalent, and next expected date.
- Combined estimated monthly and annual commitment across confirmed items plus visible suggestions, without double-counting.
- A confirmed list with manual **Add** and per-item edit/delete.

**Quick check:**

- Seed a clean recurring pattern's worth of test transactions, confirm it's suggested on the correct page, Keep it, confirm it moves to the confirmed list and totals update correctly without double-counting.
- Remove your test data afterward.

---

## Step 7: Budgets & Goals pages

**Prompt:**

Build the real Budgets and Goals screens in `apps/web`, replacing placeholders:

- **Budgets**: empty state + **Create budget** (category, monthly limit, active). Per-budget spent/limit/remaining/progress bar/over-budget state computed from real transactions in the current month, plus a budget-health summary ring. Working edit/delete via an **Adjust budgets** flow.
- **Goals**: empty state + **Create goal** (name, target amount, current amount, optional due date/note). Progress bar, remaining amount, edit/delete.

Both persist through `PUT /api/v1/preferences` without disturbing unrelated settings.

**Quick check:**

- Create a temporary budget and goal, confirm the math is correct, edit and delete them, confirm nothing else in preferences changed.

---

## Step 8: Rules & Tags page + SMS rule authoring

**Prompt:**

Build the real Rules & Tags screen in `apps/web`, replacing its placeholder:

- Categorization rules as `When … then …` rows with an enable toggle, edit, delete; a **Create rule** modal (`whenText`, `thenText`, enabled) — no seeded examples, applies to future imports only after duplicate detection.
- Tag management: all tags with usage counts, name-only **Create tag**, delete with confirmation (ask before stripping a removed tag from historical transactions).
- An **SMS Parsing Rules** subsection using the backend's `/api/v1/sms-rules` CRUD from Master Prompt A: full Add/Edit/Delete/Enable-Disable, plus a **Test rule** box where pasting a sample SMS body shows the fields it would extract. This is purely an authoring surface — the web app has no SMS access and applies these rules to nothing itself; it exists so rules can be written comfortably on a full keyboard and will sync down to the Android app (Master Prompt B) once it's built. (If you implement the actual regex-matching preview here, keep the matching logic in a plain Rust function you can point at from a test, not duplicated ad hoc in the UI — later, Master Prompt B's `crates/sms_parser` should ideally reuse or mirror this exact matching behavior.)

**Quick check:**

- Create/edit/delete a temporary rule and tag.
- Create a temporary SMS parsing rule, use "Test rule" against a sample SMS body, confirm the extracted fields look right, then delete the rule.

---

## Step 9: Settings page

**Prompt:**

Build the real Settings screen in `apps/web`, replacing its placeholder:

- **Net worth setup**: editable assets/liabilities totals, live preview of assets − liabilities, explicit note that this isn't derived from income minus expenses; saves `net_worth_configured = true`.
- **Managed categories/accounts/tags**: add/remove with confirmation, case-insensitive dedupe, block deleting something an open form currently needs.
- **Automatic detection**: plain-language explanation of recurring/subscription detection, ignored-suggestions count, working **Restore ignored suggestions**.
- **Mobile pairing**: list existing paired devices (label, last-used date), a **Generate pairing token / QR code** action calling `POST /api/v1/auth/device-tokens` from Master Prompt A — show the raw token/QR exactly once, never re-displayable after leaving the screen.
- **Danger zone**: **"Erase all MyLedger data"** requiring the exact typed confirmation `DELETE` before the destructive button enables, explaining that server records and stored files are deleted but nothing on a paired phone's own SMS inbox is touched, calling the backend's wipe endpoint, and resetting the UI to empty states on success.

**Quick check:**

- Configure and un-configure net worth; confirm persistence and the live preview both work.
- Generate a pairing token once, confirm it's shown exactly once and can't be viewed again afterward.
- Run the Danger Zone wipe against test data and confirm it empties the app correctly and doesn't sign you out.

---

## Step 10: Responsive polish, full verification & completion report

**Prompt:**

Do a complete pass over the whole MyLedger web app before calling it done:

1. Test at ~1440px desktop, ~768px tablet, and ~390px mobile-browser widths on every page. Fix any clipped text, overlapping labels, unreachable bottom-nav tabs, non-scrolling oversized modals, or horizontal page overflow you find.
2. First load: no sample financial data anywhere; Net Worth says "Not set"; period defaults to "All time".
3. Add one temporary manual transaction, refresh, confirm it persisted; delete it.
4. Add/remove a temporary tag without creating a category.
5. Confirm inline category/tag edits persist after a refresh.
6. Re-run the CSV duplicate-detection check from Step 4 once more end-to-end.
7. Upload and then delete one temporary test document.
8. Set and then unset net worth totals.
9. Create/edit/delete a temporary budget, goal, subscription, recurring item, rule, category, account, and SMS parsing rule.
10. Confirm the wipe flow requires the exact confirmation and actually empties the database.
11. Click through every nav tab, filter, modal, and Settings control at least once.
12. Grep the whole `apps/web` source for anything SMS/telephony-related and confirm there's nothing beyond the rule-authoring text fields from Step 8 — no permission requests, no device APIs.
13. Remove every temporary test artifact before finishing; leave the app in the exact empty-start state, period set to "All time".

Then give me a concise handoff: how to build and run the server locally (and roughly what's involved if I want it reachable outside my home network later), confirmation the app starts empty, a short explanation of duplicate detection and recurring/subscription detection, an honest summary of what the document-extraction pipeline can and can't do today, confirmation every tab/button was exercised across all three widths, and how to generate a mobile pairing token whenever I'm ready to run Master Prompt B.

**Quick check:**

- Read the completion report. If anything is flagged as incomplete or a known limitation, decide whether to send it back for another pass now or accept it as a known follow-up.
