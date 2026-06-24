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

### Installation
*Instructions on how to install from the Zed Extension Store will be added here once released.*

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
