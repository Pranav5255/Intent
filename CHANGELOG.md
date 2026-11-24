# Changelog

All notable changes to Intent are listed here. The newest changes come first.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Intent
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Intent 2.0 is a rewrite of 1.x in Rust (the daemon, the CLI and the pipeline) and
Tauri (the app). It reads 1.x data and accepts the same companion events.

### Added

- A Chrome companion. It records Chrome, Brave, Edge and Arc tabs the way the Firefox
  add-on does, and it skips Incognito windows. ([#30])
- macOS support. The daemon, the CLI and the app run natively. Focus comes from the
  window server and idle time from the input state. `intentctl enable` installs a
  launchd agent. ([#27])
- A Homebrew formula (`brew install --HEAD intent`); the repository is its own tap.
  A script generates the `intent-app` cask from release disk images. ([#27])
- Resume on macOS opens VS Code, Cursor or VSCodium, Firefox or the default browser,
  and iTerm or Terminal. ([#27])
- User-behaviour scenarios that act like a user, a hostile web page or a broken
  environment. CI runs them against the macOS build. ([#18], [#27])
- The 2.0 daemon `intentd`, with a single ingest path, a local HTTP API on
  `127.0.0.1:9477` and a bearer token for every route that reads captured data.
- The 2.0 desktop app (Tauri 2), with a tray icon, a command palette and
  close-to-tray.
- The 2.0 `intentctl` CLI, with a fast sender for the bash and zsh hooks.
- Search over intents with SQLite full-text search and optional vectors (sqlite-vec).
- Local-first labelling with a local LLM by default. A cloud provider is used only
  when the user configures one.
- A GNOME Shell extension that reports the focused window on Wayland.
- Test gates for every pull request: format, lint, tests, audits, and the `.deb` on
  amd64 and arm64. ([#4])

### Changed

- The desktop app shows Firefox and Chrome as one browser companion. ([#30])
- Intent ids stay the same when the pipeline runs again for a day. Before, every new
  event changed every id, so open views and links broke. ([#21])
- Today, the Now rail and the Timeline reload while they are open. Search and the
  palette show a real error when the daemon is down, not "Nothing matched". ([#19])
- Dates and times use `jiff` in place of `chrono`, which is no longer maintained. ([#25])

### Removed

- The 1.x Python services and the Electron app.

### Fixed

- Resume on macOS no longer runs the Cursor Agent CLI in place of the Cursor editor,
  and `intentctl enable` installs the editor companion through each app's own CLI. ([#29])
- The palette opens centred on X11 and takes keyboard focus. ([#26])
- A typo in the blocklist no longer stops capture, and a deleted or recreated
  workspace folder is watched again when it comes back. ([#24])
- `intentctl workspace remove` fails when nothing matches. Before, it exited 0 and
  kept the entry. ([#24])
- "Forget a day", "Forget a project" and retention delete everything they say they
  delete: raw events, intents, search rows, vectors and cached results. ([#20])
- Restores no longer run twice when you click again. A retry reopens only what failed
  the first time. ([#19])
- A search such as `axum..routing` reaches the daemon. ([#19])

### Security

- `url` is updated to 2.5.9. Version 2.5.8 could crash on a nested `blob:` URL
  (RUSTSEC-2026-0592). The app's `cookie` is updated to 0.18.3, which keeps private
  cookie nonces unique after a fork (RUSTSEC-2026-0594). ([#34])
- A Chrome tab title that is the page's own address no longer keeps the query string.
  Chrome titles a loading tab with its full address. ([#30])
- Requests from other websites cannot change Intent's data. A write with an `Origin`
  must come from a companion or the app. ([#23])
- A DNS-rebinding page cannot read Intent's API: the daemon accepts only loopback host
  names. ([#23])
- Intent rejects timestamps more than a day ahead and pauses that end in the past. ([#23])
- Secrets are redacted in shell commands, pasted text, file excerpts and window titles.
  Private-browsing window titles become "Private window". ([#22])
- Forgotten data is overwritten on disk, not only marked as deleted. ([#20])
- Workspaces cannot be `/`, the home folder or Intent's own folders. ([#24])

## [1.0.0] - 2026-07-25

The first release: Python services, an Electron app, the Firefox and VS Code
companions, and a `.deb` for Ubuntu.

[Unreleased]: https://github.com/Pranav5255/Intent/compare/b42fe8e...HEAD
[1.0.0]: https://github.com/Pranav5255/Intent/tree/b42fe8e
[#4]: https://github.com/Pranav5255/Intent/pull/4
[#18]: https://github.com/Pranav5255/Intent/pull/18
[#19]: https://github.com/Pranav5255/Intent/pull/19
[#20]: https://github.com/Pranav5255/Intent/pull/20
[#21]: https://github.com/Pranav5255/Intent/pull/21
[#22]: https://github.com/Pranav5255/Intent/pull/22
[#23]: https://github.com/Pranav5255/Intent/pull/23
[#24]: https://github.com/Pranav5255/Intent/pull/24
[#25]: https://github.com/Pranav5255/Intent/pull/25
[#26]: https://github.com/Pranav5255/Intent/pull/26
[#27]: https://github.com/Pranav5255/Intent/pull/27
[#29]: https://github.com/Pranav5255/Intent/pull/29
[#30]: https://github.com/Pranav5255/Intent/pull/30
[#34]: https://github.com/Pranav5255/Intent/pull/34
