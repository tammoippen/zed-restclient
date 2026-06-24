# REST Client 🚀

[![CI](https://github.com/doani/zed-restclient/actions/workflows/ci.yml/badge.svg)](https://github.com/doani/zed-restclient/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A REST Client extension for the [Zed editor](https://zed.dev/), bringing the powerful and intuitive workflow of `vscode-restclient` to the Zed ecosystem.

This extension allows you to send HTTP requests directly from your `.http` or `.rest` files, supporting a fast, text-based API development experience within Zed.

## ✨ Key Features (Planned & In Progress)
- [x] **HTTP Syntax Highlighting**: Full support for `.http` and `.rest` files.
- [x] **In-Editor Requests**: Send requests directly from the editor using Code Lenses.
- [ ] **Response Preview**: View beautiful, formatted JSON, XML, or HTML responses.
- [x] **Variables & Environments**: Manage dynamic data across multiple requests.
- [x] **Sidecar Architecture**: Leveraging a Rust-based native backend for high performance and reliability.

## 🗺️ Roadmap / Planned Features
- **Vertical Split View**: Automatically open HTTP responses in a vertical split below the current request tab.
- **Tab Reuse**: Reuse the same response tab for subsequent requests instead of opening new ones.
- **Response Formatting**: JSON Pretty Printing and cleaning up unnecessary headers for a cleaner output.
- **Advanced Configuration**: Configure responses via Zed's `settings.json` or a local `.restclient` config file.
- **Comprehensive Documentation Website**: A dedicated, modern VitePress documentation site covering detailed usage guides, advanced workflows, and comprehensive setup instructions.
- **Environment Support (.env)**: Support for `.env` files to manage environment-specific variables and secrets.
- **GraphQL Support**: Native support for GraphQL queries including specific formatting and parsing.

## 🚀 Getting Started

### Architecture (read this first)

This extension has **two parts**:

1. **The Zed extension** (`src/lib.rs`) — a small WebAssembly module. It does *not* make any HTTP requests itself. Its only job is to register the `.http`/`.rest` language and to launch the sidecar as a language server.
2. **The sidecar** (`sidecar/`) — a native Rust LSP binary that does the actual work: parsing your request, sending it with `reqwest`, and showing the response in a new tab.

By default, the published extension downloads a **prebuilt sidecar binary** from the upstream GitHub Releases (`doani/zed-restclient`) the first time it runs. If you want to run *only* code you have built yourself, follow the **"Build everything from source"** path below, which removes that download step.

### Prerequisites

- A recent [Rust toolchain](https://rustup.rs/) (tested with 1.95).
- The WASM target Zed uses to compile extensions:
  ```sh
  rustup target add wasm32-wasip1
  ```
- Zed (this guide was verified with the Homebrew `zed-preview` build).

### Build the sidecar from source

From the repository root:

```sh
cargo build -p sidecar --release
```

This produces the LSP binary at `target/release/sidecar`. You can sanity-check it:

```sh
cargo test -p sidecar   # runs the parser / http-client unit tests
```

### Install in Zed

Zed compiles the WASM extension for you when you install it as a **dev extension** — there is no CLI flag for this, so use the command palette:

1. Open Zed.
2. Open the command palette (`Cmd-Shift-P`) and run **`zed: install dev extension`**.
3. Select this repository's root folder (`zed-restclient/`).

Zed will build `src/lib.rs` to WASM and load the extension. Open any `.http` or `.rest` file and you should get syntax highlighting plus a **▶ Send Request** Code Lens.

> **Important:** Zed has Code Lens rendering **disabled by default**, so the **▶ Send Request** button will not appear until you enable it. Add the following to your Zed `settings.json`:
> ```json
> "code_lens": "on"
> ```
> (or run **`editor: toggle code lens`** from the command palette). Without this, the extension loads and the LSP shows green, but no run button is shown.

> On first use the extension downloads the matching prebuilt sidecar binary from GitHub Releases into Zed's extension work directory. If you prefer to run your own build, do the next step instead.

### (Recommended for full trust) Run your own locally-built sidecar

To avoid the runtime download entirely and run only the sidecar **you** compiled, point the extension at your local binary before installing it. Edit `language_server_command` in `src/lib.rs` so it launches your build instead of downloading one:

```rust
fn language_server_command(
    &mut self,
    _language_server_id: &zed::LanguageServerId,
    _worktree: &zed::Worktree,
) -> Result<zed::Command> {
    Ok(zed::Command {
        // Absolute path to the binary produced by `cargo build -p sidecar --release`
        command: "/ABSOLUTE/PATH/TO/zed-restclient/target/release/sidecar".to_string(),
        args: vec![],
        env: vec![],
    })
}
```

Then run **`zed: install dev extension`** (or **`zed: rebuild dev extension`** if it is already installed). Now no binaries are fetched from the network — the only HTTP traffic is the requests you trigger yourself.

### Usage
Create a file ending in `.http` or `.rest` and write your request.

**Important formatting rules for the "Send Request" button to appear:**
1. You can separate multiple requests using `###` (optionally followed by a name/comment).
2. **Crucial:** You must leave at least one **blank line** between the `###` separator (or the top of the file/variables) and the actual request line (e.g., `GET ...`) to allow the Code Lens to be rendered correctly!

```http
@baseUrl = https://api.github.com

### Get Repository Info

GET {{baseUrl}}/repos/doani/zed-restclient
Accept: application/json
```

Then click the **▶ Send Request** button (Code Lens) that appears directly above the `GET` line.

#### Variables

- **File variables** — define with `@name = value` and use as `{{name}}`.
- **System variables** — `{{$guid}}`, `{{$datetime}}` (also `{{$datetime rfc1123}}` / `{{$datetime iso8601}}`), and `{{$randomInt min max}}`.
- **Environment variables**:
  - `{{$dotenv NAME}}` reads `NAME` from the nearest `.env` file, searched from the request file's directory upwards. The closest `.env` wins.
  - `{{$processEnv NAME}}` reads `NAME` from the process environment of the running editor.

These compose with file variables, so a common pattern is:

```http
@token = {{$dotenv ACCESS_TOKEN}}
@user  = {{$processEnv USER}}

### Get current user

GET https://api.example.com/me
Authorization: Bearer {{token}}
X-User: {{user}}
```

A matching `.env` next to the file:

```dotenv
ACCESS_TOKEN=eyJhbGciOi...
```

### Response output

The response opens in a scratch buffer. By default it contains only the response. You can optionally show the **resolved request** above it so you can see exactly what was sent:

```http
GET https://api.example.com/me
authorization: Bearer abc123

###  Response  ###

HTTP/1.1 200 OK
content-type: application/json

{ "id": 1 }
```

Enable it (`showRequest`, default `false`) in either of two ways:

- **Zed settings** (`settings.json`) via the language server's initialization options:
  ```json
  "lsp": {
    "rest-client": {
      "initialization_options": {
        "showRequest": true
      }
    }
  }
  ```
- **Environment variable** (handy when running a locally-built sidecar): set `ZED_RESTCLIENT_SHOW_REQUEST` to `true`/`1`/`on` (or `false`/`0`/`off`). The `settings.json` value takes precedence over the environment variable.

> ⚠️ When enabled, the request block is rendered **verbatim**, including `Authorization` and other secret headers in cleartext.

### How the response opens

The response is written to a scratch `.http` file and opened in your **running Zed window**, reusing the same tab on subsequent requests. The sidecar finds the Zed CLI automatically — first the running instance (via its process), then `PATH`, then well-known install locations — so it no longer depends on `PATH` or the `.http` file association. If discovery ever fails, set `ZED_RESTCLIENT_ZED_BIN` to the absolute path of your Zed CLI.

> **Pane placement:** Zed's CLI and LSP protocol provide no way for an external process to target a specific pane or force a split, so the response opens in the **active pane** of the current window. Move the response tab into a split once; because the tab is reused, later sends update it in place.

## 🤝 Contributing

Contributions are what make the open-source community such an amazing place to learn, inspire, and create. Any contributions you make are **greatly appreciated**.

**Important Rule: No Feature Without an Issue.**
Before starting work on a new feature or bug fix, **please open an issue first** to discuss the idea.

1. Open an Issue
2. Fork the Project
3. Create your Feature Branch (`git checkout -b feature/issue-123-AmazingFeature`)
4. Commit your Changes (`git commit -m 'feat: Add some AmazingFeature'`)
5. Push to the Branch (`git push origin feature/issue-123-AmazingFeature`)
6. Open a Pull Request

See [CONTRIBUTING.md](CONTRIBUTING.md) for more details.

## 📜 License
Distributed under the MIT License. See `LICENSE` for more information.
