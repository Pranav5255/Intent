# Intent on macOS

The daemon, the CLI and the app run natively on macOS (Apple silicon and Intel). The
pipeline, the store, search, the local LLM and the HTTP API are the same code as on
Ubuntu. Only the parts that touch the desktop differ.

| Part | Ubuntu | macOS |
|---|---|---|
| Focused window | X11 events or the GNOME extension | The window server's front window (`CGWindowList`), sampled every 2 s while active, up to 10 s while idle |
| Window titles | Always | Need the **Screen Recording** permission for `intentd`; without it Intent records the app only |
| Idle | Mutter idle monitor or MIT-SCREEN-SAVER | Seconds since the last input (`CGEventSource`), no permission |
| Files in workspaces | inotify | FSEvents |
| Shell commands | bash and zsh hooks | The same hooks (zsh is the default shell) |
| Resume | `code`, Firefox or `xdg-open`, GNOME Terminal and friends | `code` if on `PATH`, else `open -a` for VS Code, Cursor or VSCodium; Firefox or the default browser; iTerm or Terminal |
| Service | systemd user unit | launchd agent `~/Library/LaunchAgents/io.github.pranav5255.intentd.plist`, logs in `~/Library/Logs/Intent` |
| Palette shortcut | Super+Space | Option+Space (Command+Space is Spotlight) |
| Install | `.deb` | Homebrew formula and cask |

Focus events keep the source name `linux` on macOS, so the engine, redaction and stored
data stay the same on both systems.

## Install with Homebrew

The repository is its own tap.

```bash
brew tap pranav5255/intent https://github.com/Pranav5255/Intent
brew install --HEAD intent      # builds intentd and intentctl from source (about 2 minutes)
intentctl enable                # start at login and add the shell hook
intentctl status
```

`brew test intent` starts the daemon on a free port and checks `/healthz`.

The app is the `intent-app` cask. It depends on the formula. A release must publish
`Intent_<version>_aarch64.dmg` and `Intent_<version>_x64.dmg`. Then
`packaging/homebrew/make-cask.sh <version> <arm64.dmg> <x64.dmg> > Casks/intent-app.rb`
writes the cask with their checksums.

## Limits

- **Signing.** A build without an Apple Developer ID is only ad-hoc signed. Gatekeeper
  refuses it after a download, and macOS forgets the Screen Recording permission each
  time the binary changes. A public release needs a Developer ID and notarization.
  For a local build, run `xattr -dr com.apple.quarantine /Applications/Intent.app`.
- **Window titles.** Without the Screen Recording permission, threads group by app only.
- **Browsers.** Only Firefox has a companion. Safari and Chrome tabs appear as window
  titles only.
- **Notification.** The "Continue yesterday?" notification is Linux-only.
- **Accent colour.** The app reads the GNOME accent colour only.
- **Port 9477.** The Lima VM forwards its daemon's port to the Mac. Stop the VM
  (`limactl stop intent`) before you run Intent on the Mac itself.

## Tests

The `package-macos` CI job builds the daemon and the app on macOS. It runs the
user-behaviour scenarios and the shell-hook tests, and checks that the collectors attach.
It then installs the formula and the cask from the checkout. Locally:

```bash
cargo build --release -p intentd -p intentctl
INTENTD=$PWD/target/release/intentd INTENTCTL=$PWD/target/release/intentctl dev/lima/scenarios.sh
cargo test -p intent-collectors -- --ignored macos_ --nocapture   # live focus and idle readings
```
