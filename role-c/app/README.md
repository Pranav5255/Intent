# Intent Role C

Electron + React desktop overlay for the local Role A and Role B services. It is intentionally a presentation layer: all work history comes from Role B, and every Role A restore is reviewed and confirmed by the user.

## Run

```bash
cd role-c/app
npm install
npm run dev
```

The Electron overlay uses `Ctrl+Space` to open or close. In a regular browser, the same shortcut works while the page has focus.

Start Role B on `127.0.0.1:9478` and Role A on `127.0.0.1:9477` for live data and restores. The role URLs can be changed for development with `VITE_ROLE_B_URL` and `VITE_ROLE_A_URL`.

To inspect the UI with bundled local data and no services, visit the Vite server with `?mock=1`, for example `http://127.0.0.1:9479/?mock=1`.

To use the checked-in golden fixture on its historical date, run Role C with `VITE_INTENT_DATE=2026-07-13`; it then requests that date from Role B rather than the local previous calendar day.

## Controls

- Plain text or `/query`: search saved sessions.
- `?question`: ask the optional Role B Copilot.
- `!query`: resolve a stored session for restore.
- `↑` / `↓`: move through intent rows.
- `Enter`: open the selected session’s restore review, or submit a Copilot question.
- `Esc`: close the review or overlay.

The review panel displays only the stored Role B payload and adds just the selected `resume` or `continue` mode before posting it to Role A.

## Build

```bash
npm run build
npm run package:linux
```

`package:linux` creates Ubuntu-oriented AppImage and Debian package targets through electron-builder.

## Verify the overlay and restores

Run the automated checks without opening applications:

```bash
npm test
```

These tests cover the full-screen transparent window configuration, the local-only API bridge, Role B resume selection, and Role A request body. The Role A restore tests can also be run directly from `role-a`:

```bash
.venv/bin/python -m unittest event_server.tests.test_restore -v
```

They mock the `code`, `firefox`, and `gnome-terminal` launchers and verify the exact Ubuntu commands that a confirmed restore would use.

For a visual no-risk overlay check, start with local mock data. It opens no external windows:

```bash
VITE_MOCK_MODE=true npm run dev
```

Press `Ctrl+Space` from the desktop to show the overlay, search for `login`, and choose Resume or Continue. In mock mode the review and success flow is exercised but no HTTP restore request is made.

For a live restore check, start Role A and Role B, seed or capture an intent whose stored files and terminal directory exist on this machine, then start Role C normally. The confirmed action calls Role A, which uses `code --reuse-window` for existing files, Firefox for HTTP(S) URLs, and GNOME Terminal at the stored working directory. The stored `last_cmd` is never executed.

For an immediately runnable live launcher check on this repository, replay `fixtures/live-restore-demo.json` into Role B. It stores two real documentation URLs and `~/Intent-OS/role-c/app` as the terminal directory, so confirming Restore opens Firefox and GNOME Terminal but does not rely on VS Code being installed. Use `VITE_INTENT_DATE=2026-07-20` when starting Role C to view that historical demo date.
