# Plan: Request Variables (named requests & response chaining)

Status: **Phase 1 implemented** (naming + cache + headers + full body + JSONPath)
Branch: `feat/request-variables`
Tracking the vscode-restclient ["Request Variables"](https://github.com/Huachao/vscode-restclient#request-variables) feature.

**Decision:** Phase 1 uses **lazy** evaluation (match vscode-restclient exactly) —
named requests are sent manually; references to an un-run request send literal
text plus a diagnostic. Eager auto-run of dependencies is explicitly out of scope
(possible future enhancement).

## 1. Goal

Let a request be *named* and let later requests (or file variables) reference the
named request's **request** or **response** data:

```http
@baseUrl = https://example.com/api

# @name login
POST {{baseUrl}}/login
Content-Type: application/json

{ "name": "foo", "password": "bar" }

###

@authToken = {{login.response.headers.X-AuthToken}}

# @name createComment
POST {{baseUrl}}/comments
Authorization: {{authToken}}
Content-Type: application/json

{ "content": "hello" }

###

@commentId = {{createComment.response.body.$.id}}

GET {{baseUrl}}/comments/{{commentId}}
Authorization: {{authToken}}
```

### Reference grammar (must match vscode-restclient)

```
{{ <name> . (request|response) . (body|headers) . <accessor> }}
```

| Segment      | Values |
| ------------ | ------ |
| `name`       | the `# @name <name>` of a previously **executed** request |
| message      | `request` or `response` |
| part         | `body` or `headers` |
| `accessor`   | for `body`: `*` (full body), a **JSONPath** (`$.a.b`, `$[0].id`), or an **XPath** (`/feed/title`); for `headers`: a **header name** (case-insensitive) |

### Behavior to match

- **Lazy / manual**: referenced named requests are **not** auto-sent. The user
  triggers them first (Send Request); their result is cached. References to a
  request that has not run yet resolve to the **literal text** (and we surface a
  diagnostic / log).
- **File scope**: cache is per `.http` document.
- **Precedence**: request variables take precedence over file/env/dotenv
  variables when a name would collide (in practice they don't collide because
  request-variable references are dotted and file-variable names are not).
- **Unresolved path**: if a JSONPath/XPath/header lookup fails, send the literal
  reference text (matches vscode-restclient).

## 2. Current state (what exists today)

- `parser.rs` — `HttpRequest { method, url, headers, body }`, `HttpFile { requests, variables }`. Lines starting with `#`/`//` are treated as comments and **discarded**, so `# @name x` is currently dropped.
- `codelens.rs` — finds request start lines; the Send button passes `(uri, block_index)`.
- `main.rs::handle_send_request` — parses the file, builds the request via `http_client::build_request`, executes it, writes the response to a temp file. **No response is retained.**
- `http_client.rs::resolve_variables` — substitutes file variables, then `$dotenv`/`$processEnv`/`$guid`/`$datetime`/`$randomInt`. No notion of request variables.
- `Backend` (in `main.rs`) holds `document_map` and `show_request`. No response cache.

So three things are missing: **names**, a **response cache**, and **reference resolution**.

## 3. Proposed design

### 3.1 Parse the name

In `parser.rs`, while in the `looking_for_request` phase, detect a name marker
before consuming comments:

- Match a trimmed line of the form `# @name <name>` or `// @name <name>`
  (also accept `# @name=<name>`). Capture `<name>`; keep skipping other comments.
- Add `name: Option<&'a str>` to `HttpRequest`.
- Reset the pending name at each `###` separator and after attaching it.

`codelens.rs` does not need the name (the Send button already identifies the
block by index); naming only matters when the request is sent and cached.

### 3.2 Response cache

New types (proposed module `sidecar/src/exchange.rs`):

```rust
pub struct ExchangeMessage {
    pub headers: Vec<(String, String)>, // preserves order; lookup is case-insensitive
    pub body: String,
}

pub struct Exchange {
    pub request: ExchangeMessage,   // as actually sent (post-resolution)
    pub response: ExchangeMessage,  // status line captured separately if needed
}
```

Store on `Backend`:

```rust
// per-document, keyed by request name
response_cache: RwLock<HashMap<Url, HashMap<String, Exchange>>>,
```

Populate it at the end of `handle_send_request`: if the sent request had a
`name`, insert/overwrite `Exchange { request, response }` for `(uri, name)`.
Capturing the **request** message requires recording the resolved method/url/
headers/body that were actually sent (we already build a `reqwest::Request`;
`render_request` from the show-request feature already extracts headers+body and
can be reused / factored).

### 3.3 Reference parsing

Add `parse_request_var_ref(inner: &str) -> Option<RequestVarRef>`:

- Split `inner` into the first three dot-separated segments: `name`, `message`,
  `part`. **The accessor is the remainder** (it may contain dots, e.g.
  `$.data.token` or `/feed/entry/title`), so split with `splitn(4, '.')`.
- Validate `message ∈ {request, response}` and `part ∈ {body, headers}`;
  otherwise return `None` (so it is treated as a normal variable).

```rust
struct RequestVarRef<'a> {
    name: &'a str,
    message: Message,   // Request | Response
    part: Part,         // Body | Headers
    accessor: &'a str,  // "*", JSONPath, XPath, or header name
}
```

### 3.4 Accessor evaluation

`fn eval_accessor(part: Part, accessor: &str, msg: &ExchangeMessage) -> Option<String>`

- **headers** → case-insensitive lookup of `accessor` in `msg.headers`.
- **body + `*`** → return `msg.body` verbatim.
- **body + JSONPath** (`accessor` starts with `$`) → parse `msg.body` as JSON,
  evaluate JSONPath, return the first match. Scalars rendered without quotes;
  objects/arrays serialized compactly.
- **body + XPath** (otherwise, e.g. starts with `/` or a node test) → parse
  `msg.body` as XML and evaluate XPath; return the string value of the first
  node.

### 3.5 Resolution integration

Add `resolve_request_variables(text, cache, &mut diagnostics) -> String` and run
it inside the existing resolution pipeline. Order within a field:

1. **File variables** (`@x`) — substitute raw values, which may themselves
   contain `{{name.response...}}` refs.
2. **Request variables** — resolve `{{name.(request|response).(body|headers).acc}}`
   from the cache.
3. **System variables** — `$dotenv`, `$processEnv`, `$guid`, `$datetime`,
   `$randomInt`.

`build_request` needs access to the per-document cache. Plumb a
`&HashMap<String, Exchange>` (the current document's cache snapshot) through
`build_request` next to `variables`/`dotenv`, mirroring how `dotenv` was added.
`main.rs` takes a read-lock, clones/borrows the document's map, and passes it in.

Unresolved refs (request not cached, or accessor not found) are left as the
literal `{{...}}` text and recorded as a diagnostic.

### 3.6 Diagnostics (nice-to-have, phase 3)

Publish `textDocument/publishDiagnostics` for unresolved request-variable refs
(e.g. "named request `login` has not been sent yet"). vscode-restclient shows
similar diagnostics and hovers. Hover support is optional and out of scope for v1.

## 4. Dependencies to add (`sidecar/Cargo.toml`)

- **JSONPath**: [`serde_json_path`](https://crates.io/crates/serde_json_path)
  (RFC 9535, actively maintained, integrates with the `serde_json` we already
  use). Alternative: `jsonpath-rust`.
- **XPath/XML**: [`sxd-document`] + [`sxd-xpath`] (pure Rust, XPath 1.0).
  XML/XPath is lower priority — gate it behind phase 2 so the common JSON case
  ships first. Avoid `libxml` (needs system libxml2, hurts portability).

## 5. Phased implementation

1. **Phase 1 — naming + cache + headers + full body + JSONPath** ✅ **done**
   (covers the motivating examples):
   - Parser: capture `# @name`. ✅
   - `Backend` response cache + populate on send (`capture_request_message`
     records the resolved request). ✅
   - Reference parser + evaluator for `headers`, `body.*`, `body.$json`
     (`sidecar/src/exchange.rs`). ✅
   - Wire into resolution pipeline; literal fallback when not cached. ✅
   - Add `serde_json_path`. ✅
   - Tests (see §6). ✅
2. **Phase 2 — XPath/XML** body accessors via `sxd-*`.
3. **Phase 3 — diagnostics/hover** for unresolved/typed references.

## 6. Testing strategy

Pure, unit-testable pieces (no network needed):

- `parse_request_var_ref`: valid/invalid forms; accessor with dots; non-refs
  (`{{plainVar}}`, `{{$guid}}`) return `None`.
- `eval_accessor`:
  - header case-insensitive hit/miss;
  - `body.*` full body;
  - JSONPath: `$.token`, nested `$.data.id`, array `$[0].id`, scalar vs object
    rendering, missing path → `None`.
- `resolve_request_variables` against a hand-built cache, including the
  file-variable indirection case `@authToken = {{login.response.headers.X-AuthToken}}`.
- Parser test: `# @name login` and `// @name login` attach the name to the next
  request; reset across `###`.
- Cache population: an integration-style test that sends to a local mock and
  asserts the named exchange is stored (or factor the cache write so it can be
  unit-tested without a socket).

## 7. Edge cases & decisions

- **Accessor with dots** — must use `splitn(4, '.')`, not `split('.')`.
- **JSON scalar rendering** — `$.token` → `abc123` (no surrounding quotes);
  object/array → compact JSON. Match vscode-restclient (which inserts the raw
  value).
- **Header name case-insensitivity** — normalize on lookup.
- **`.request.` references** — require capturing the **resolved** request
  (post-variable-substitution) message, i.e. exactly what was sent.
- **Re-running a named request** overwrites its cache entry.
- **Lazy only** — do not auto-send dependencies in v1 (matches vscode); note as
  a possible future enhancement (auto-run the dependency chain).
- **Document edits / `did_change`** — keep cached exchanges until the request is
  re-sent or the document is closed (`did_close` clears its cache entry).
- **`show_request` interaction** — the request preview already shows the
  resolved request; ensure request-variable resolution happens before the
  preview is captured so the preview reflects substituted values.

## 8. Open questions

- Multiple JSONPath matches: vscode returns the first; confirm and document.
- Should unresolved refs block the send or send-with-literal? vscode sends the
  literal; we will do the same and add a diagnostic.
- XPath default namespace handling (sxd-xpath needs explicit namespace
  registration) — likely punt to phase 2 with documented limitations.
