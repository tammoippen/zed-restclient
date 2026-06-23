# Plan: Hover support for variables

Status: **Design / not yet implemented**
Branch (suggested): `feat/variable-hover`
Tracking vscode-restclient's hover behaviour, where mousing over a `{{...}}`
placeholder shows what it resolves to.

## 0. Who this is for

This plan assumes **no prior knowledge** of this code base or of Zed
extensions. Read §1–§4 for orientation, then implement §6 step by step. §7
lists the exact commands to run, and §8 is the working discipline
(red-green TDD, small commits) you must follow.

---

## 1. Goal & UX

When the user hovers the mouse over (or places the cursor in) a `{{...}}`
placeholder in a `.http` file, show a popup describing what that placeholder
resolves to. This must work for **all three kinds** of variable:

| Kind | Example placeholder | Hover should show |
| ---- | ------------------- | ----------------- |
| **File variable** | `{{baseUrl}}` | the defined value, e.g. `https://api.example.com`, plus its fully-resolved value if it contains nested variables |
| **System variable** | `{{$guid}}`, `{{$datetime iso8601}}`, `{{$randomInt 1 10}}` | a short description and a freshly-generated example value |
| **Request variable** | `{{login.response.body.$.token}}` | the resolved value from the cached response, or a message explaining it has not been sent / could not resolve |

If the cursor is **not** inside a `{{...}}` placeholder, hover returns nothing
(no popup).

---

## 2. Architecture & where things live

The project has **two** crates:

```
zed-restclient/
├── Cargo.toml            # workspace root; builds the Zed extension (wasm)
├── src/lib.rs            # the Zed extension: only LAUNCHES the sidecar
└── sidecar/
    ├── Cargo.toml
    └── src/
        ├── main.rs       # the LSP server (tower-lsp). All LSP handlers live here.
        ├── parser.rs     # parses .http text into requests + file variables
        ├── http_client.rs# builds/sends requests; resolves variables
        ├── codelens.rs   # "▶ Send Request" code lenses
        └── exchange.rs    # request-variable engine (cache, refs, resolution, diagnostics)
```

**Key fact:** Hover is a standard LSP request (`textDocument/hover`). It is
handled **entirely inside the sidecar** (`sidecar/src/main.rs`). The Zed
extension (`src/lib.rs`) forwards hover to the sidecar automatically once the
server advertises the capability — **you do not touch `src/lib.rs` or any wasm
code.**

The sidecar uses [`tower-lsp`](https://docs.rs/tower-lsp) `0.20`. The server is
the `Backend` struct in `main.rs`, which implements
`tower_lsp::LanguageServer`. It already holds:

```rust
struct Backend {
    client: Client,
    document_map: RwLock<HashMap<Url, String>>,         // uri -> current file text
    response_cache: RwLock<HashMap<Url, exchange::ExchangeCache>>, // uri -> named exchanges
}
```

Relevant existing helpers you will reuse:

- `main.rs::offset_to_position(text: &str, offset: usize) -> Position` — converts
  a **byte offset** into an LSP `Position` (line + **UTF-16** character). You
  will add its inverse.
- `parser::parse_http_file(content: &str) -> HttpFile` — `HttpFile.variables`
  is a `HashMap<&str, &str>` of file variables (`@name = value`).
- `exchange::parse_request_var_ref`, `exchange::eval_accessor`,
  `exchange::ExchangeCache` — the request-variable machinery.
- `http_client.rs` — `resolve_system_variables` (private) and the variable
  resolution pipeline (private `resolve_variables`).

---

## 3. Variable kinds — exact syntax (ground truth from the code)

All placeholders are delimited by `{{` and `}}`. Inside:

1. **Request variables** — recognised by `exchange::parse_request_var_ref`:
   `name.(request|response).(body|headers).accessor`
   (e.g. `login.response.headers.X-AuthToken`,
   `login.response.body.$.data.token`, `feed.response.body./feed/title`).

2. **System variables** — begin with `$`. Implemented in
   `http_client.rs::resolve_system_variables`:
   - `{{$guid}}`
   - `{{$datetime rfc1123}}`
   - `{{$datetime iso8601}}`
   - `{{$datetime}}` (defaults to iso8601)
   - `{{$randomInt min max}}` (also `{{$randomInt max}}` / `{{$randomInt}}`)

3. **File variables** — everything else: a bare name defined elsewhere with
   `@name = value`. Looked up in `HttpFile.variables`.

**Classification order matters** (a name like `$guid` must not be treated as a
file variable): test **request var → system var (`starts_with('$')`) → file
var**.

---

## 4. Design

Create a **new pure module** `sidecar/src/hover.rs` that holds all the
hover *logic* with no LSP/network dependencies, so it is fully unit-testable.
`main.rs` will contain only a thin adapter that does I/O (read document, read
cache) and calls into `hover.rs`.

### 4.1 Placeholder location

```rust
/// A `{{...}}` placeholder located by byte offsets, with its trimmed inner text.
pub struct Placeholder<'a> {
    pub start: usize,   // byte offset of '{{'
    pub end: usize,     // byte offset just past '}}'
    pub inner: &'a str, // trimmed text between the braces
}

/// The placeholder whose `{{...}}` span contains `offset`, if any.
pub fn placeholder_at(text: &str, offset: usize) -> Option<Placeholder<'_>>;
```

Note: `exchange.rs` already scans `{{`/`}}` in three places
(`resolve_request_variables`, `collect_unresolved_refs`). Consider extracting a
single shared scanner during this work (optional refactor — see §6 step 6), but
it is acceptable for `placeholder_at` to do its own scan.

### 4.2 Building the hover text

```rust
/// Markdown body for the placeholder `inner`, classified across all three
/// variable kinds. Returns None if `inner` matches nothing we can describe.
pub fn hover_markdown(
    inner: &str,
    variables: &HashMap<&str, &str>,
    cache: &ExchangeCache,
) -> Option<String>;
```

Behaviour by kind:

- **Request var** (`parse_request_var_ref(inner)` is `Some`): resolve against
  `cache`. On success show the value in a fenced code block; on failure show the
  same human message the diagnostics use ("named request `login` has not been
  sent yet" / "could not resolve `…`").
- **System var** (`inner.starts_with('$')`): show a one-line description from a
  static table, plus `Example: <freshly generated value>` (generate by feeding
  `{{inner}}` through the system-variable resolver).
- **File var** (otherwise): look up `variables[inner]`. Show the raw value and,
  if it differs, the fully-resolved value (after nested file/request/system
  substitution). If the name is undefined, show "Undefined variable `name`".

### 4.3 Shared resolution helpers (refactor, don't duplicate)

To keep hover and the existing features consistent, expose small helpers and
have **both** call sites use them:

- In `exchange.rs`, add
  `pub fn resolve_ref(reference: &RequestVarRef, cache: &ExchangeCache) -> Result<String, String>`
  (`Ok(value)` / `Err(human_reason)`), and refactor `collect_unresolved_refs`
  to build its messages from the `Err` case so diagnostics and hover never
  disagree.
- In `http_client.rs`, expose the existing private resolution as
  `pub fn resolve_all(text, variables, cache) -> String` (or expose
  `resolve_system_variables`) so hover can compute "example"/"resolved" values
  without re-implementing the logic.

### 4.4 The LSP adapter (`main.rs`)

```rust
// inverse of offset_to_position; UTF-16 aware.
fn position_to_offset(text: &str, pos: Position) -> usize;

// in `impl LanguageServer for Backend`:
async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
    // 1. uri + position from params.text_document_position_params
    // 2. read text from document_map; read per-uri cache from response_cache
    // 3. let file = parser::parse_http_file(&text);
    // 4. let offset = position_to_offset(&text, position);
    // 5. let ph = hover::placeholder_at(&text, offset) else return Ok(None);
    // 6. let md = hover::hover_markdown(ph.inner, &file.variables, &cache) else return Ok(None);
    // 7. build Hover { contents: HoverContents::Markup(MarkupContent{ kind: Markdown, value: md }),
    //                  range: Some(Range{ offset_to_position(start), offset_to_position(end) }) }
}
```

And advertise the capability in `initialize`:

```rust
hover_provider: Some(HoverProviderCapability::Simple(true)),
```

---

## 5. Why the logic lives in `hover.rs`

The `hover` LSP handler needs the `tower_lsp::Client` and async I/O, which makes
it awkward to unit-test. Keep `hover` a thin adapter and put every decision in
pure functions (`placeholder_at`, `hover_markdown`, `position_to_offset`,
`resolve_ref`). Those get full test coverage; the adapter is exercised manually
in Zed (§7.3).

---

## 6. Phased implementation (do these in order, each its own commit)

> Follow the **modus operandi** in §8 for every step: write the failing test
> first (RED), implement until green (GREEN), refactor, run the full check
> suite, commit.

### Step 1 — `position_to_offset` (`main.rs`)
- **RED:** add `#[cfg(test)]` tests in `main.rs`:
  - round-trips with `offset_to_position` on a multi-line string;
  - a line with a multi-byte char (`é`) maps an LSP character back to the right
    byte offset;
  - a `character` past end-of-line clamps to the line end;
  - a `line` past end-of-file clamps to text length.
- **GREEN:** implement `position_to_offset(text, pos) -> usize` (walk lines to
  `pos.line`, then advance `pos.character` UTF-16 units within that line).
- Commit: `feat(main): add position_to_offset (UTF-16 aware)`.

### Step 2 — `hover.rs` skeleton + `placeholder_at`
- Create `sidecar/src/hover.rs`; add `mod hover;` to `main.rs`.
- **RED:** tests for `placeholder_at`:
  - cursor inside `{{baseUrl}}` returns the span + `inner == "baseUrl"`;
  - cursor on the `{{` and on the `}}` edges still match;
  - cursor between two placeholders returns `None`;
  - cursor outside any braces returns `None`;
  - unterminated `{{` returns `None`.
- **GREEN:** implement `Placeholder` + `placeholder_at`.
- Commit: `feat(hover): locate the placeholder under a byte offset`.

### Step 3 — request-variable resolution helper (refactor)
- **RED:** tests for `exchange::resolve_ref`:
  - cached header/body/JSONPath/XPath → `Ok(value)`;
  - un-run request → `Err(msg containing the name and "has not been sent")`;
  - bad accessor → `Err(msg containing the accessor)`.
- **GREEN:** add `resolve_ref`; refactor `collect_unresolved_refs` to derive its
  messages from `resolve_ref`'s `Err`. Existing diagnostics tests must still
  pass unchanged.
- Commit: `refactor(exchange): extract resolve_ref shared by diagnostics`.

### Step 4 — expose a resolution helper for system/file values (refactor)
- **GREEN (small):** make the needed function(s) in `http_client.rs` public
  (e.g. `pub fn resolve_all(...)` wrapping today's private `resolve_variables`,
  and/or `pub fn resolve_system_variables(...)`). No behaviour change; existing
  tests stay green. Add a test asserting `resolve_all` applies file → request →
  system order (mirrors `resolve_variables`).
- Commit: `refactor(http_client): expose variable resolution for reuse`.

### Step 5 — `hover_markdown` for all three kinds (`hover.rs`)
- **RED:** tests for `hover_markdown(inner, &variables, &cache)`:
  - **file var:** `variables = {baseUrl: "https://x"}`, `inner = "baseUrl"` →
    markdown contains `https://x`;
  - file var with nested ref shows both raw and resolved;
  - **undefined** file var → contains "Undefined";
  - **system var:** `inner = "$guid"` → contains a description and an example
    that looks like a UUID; `"$randomInt 1 1"` → example `1`;
  - **request var (resolved):** cache has `login`, `inner =
    "login.response.headers.X-AuthToken"` → contains the token;
  - **request var (unsent):** empty cache → contains "has not been sent".
- **GREEN:** implement classification (request → `$` system → file) and the
  static system-variable description table; build markdown via the helpers from
  steps 3–4.
- Commit: `feat(hover): render hover markdown for all variable kinds`.

### Step 6 (optional) — unify the `{{...}}` scanners
- If time allows, replace the three ad-hoc scanners in `exchange.rs` and
  `placeholder_at` with one shared iterator. Pure refactor; all tests stay green.
- Commit: `refactor: share a single {{...}} placeholder scanner`.

### Step 7 — wire the LSP handler (`main.rs`)
- Add `hover_provider: Some(HoverProviderCapability::Simple(true))` to
  `ServerCapabilities` in `initialize`.
- Implement `async fn hover(...)` as the thin adapter in §4.4.
- This is integration glue (needs the live `Client`), so it is verified
  manually in Zed (§7.3) rather than by unit test.
- Commit: `feat: implement textDocument/hover for variables`.

### Step 8 — docs
- Update this file's Status to "implemented" and note hover in
  `docs/request-variables-plan.md` / `README.md` as appropriate.
- Commit: `docs: document variable hover support`.

---

## 7. Testing & verification

### 7.1 Commands (run from `sidecar/`)
```bash
cd sidecar
cargo test                       # all unit tests must pass
cargo clippy --all-targets       # must be warning-free
cargo fmt                        # format
cargo fmt --check                # confirm formatted
```
> The workspace root `cargo test` only runs the wasm extension crate. The
> sidecar tests must be run from `sidecar/` (or `cargo test -p sidecar`).

### 7.2 What to unit-test
Everything pure: `position_to_offset`, `placeholder_at`, `hover_markdown`,
`exchange::resolve_ref`, `http_client::resolve_all`.

### 7.3 Manual check in Zed
- In `src/lib.rs::language_server_command`, switch to the **local development**
  branch (the commented block) so Zed runs the sidecar via
  `cargo run --manifest-path .../sidecar/Cargo.toml`. Do **not** commit that
  switch.
- Open a `.http` file with file, system, and request variables. Hover each:
  - `{{baseUrl}}` shows its value;
  - `{{$guid}}` shows description + example;
  - a `{{name.response...}}` ref shows the value after you Send the named
    request, and the "not sent" message before.

---

## 8. Modus operandi (follow strictly)

1. **Red-Green TDD.** For each behaviour: write a failing test first, run it,
   see it fail for the right reason, then implement the minimum to make it pass.
   Refactor only with green tests.
2. **Small, focused commits** — one logical change each, as listed in §6. Keep
   commits compiling and green.
3. **Pure logic in `hover.rs` / `exchange.rs`; I/O only in `main.rs`.** Never
   put untested branching in the async `hover` handler.
4. **No duplication.** Reuse `offset_to_position`, `parse_request_var_ref`,
   `eval_accessor`, and the resolution pipeline via the shared helpers (steps
   3–4) instead of re-implementing them.
5. **Before every commit:** `cargo test` green, `cargo clippy --all-targets`
   warning-free, `cargo fmt` applied.
6. **Don't touch the Zed extension/wasm** (`src/lib.rs`) except for local manual
   testing, and never commit that change.
7. **Match existing style** — `#[allow(dead_code)]` is used on
   not-yet-wired items; remove it once an item is actually used.

---

## 9. Edge cases & decisions

- **Classification precedence:** request var → system var (`$`) → file var.
- **UTF-16 positions:** LSP `character` counts UTF-16 code units. `position_to_offset`
  and `offset_to_position` must agree; test with a non-ASCII char.
- **Cursor exactly on `{{` / `}}`:** treat the whole `{{...}}` span (start
  inclusive, end exclusive of the byte past `}}`) as hoverable.
- **Large bodies:** when a request-variable resolves to a big body (`body.*`),
  truncate in the popup (e.g. first ~2 KB + `…`).
- **System-variable examples are regenerated each hover** (`$guid`/`$datetime`/
  `$randomInt` are non-deterministic) — label them "Example", not "Value".
- **Undefined file variable:** still return a popup that says it is undefined,
  to help users catch typos.
- **Nested/recursive file variables:** show raw value and best-effort resolved
  value; do not loop infinitely (the existing resolver already does a bounded
  number of passes).
- **Hover range:** return the placeholder span as `Hover.range` so the editor
  highlights the whole `{{...}}`.
```
