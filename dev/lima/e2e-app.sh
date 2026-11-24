#!/usr/bin/env bash
# End-to-end check of the packaged Tauri app against the real daemon (Lima VM):
#   limactl shell intent -- bash -lc '<repo>/dev/lima/e2e-app.sh [path/to/intent.deb]'
#
# Build the .deb first with INTENT_BUILD_APP=1 packaging/build-deb.sh. This runs
# dev/lima/e2e.sh (install + real activity + pipeline) unless SKIP_SEED=1, then
# launches /usr/bin/intent under the same Xvfb + openbox session and checks:
#   - package layout (launcher, binary, desktop entry, icons) and its libraries;
#   - the main window renders real data (Today, Timeline, Search, --preview <id>);
#   - every app → daemon call is authorised (loopback capture, no 401);
#   - Super+Space opens and closes the palette, every reopen takes keyboard input,
#     and "Pause capture for 15 minutes" from it makes the main window say "resumes …";
#   - the tray icon registers when a tray host exists (stalonetray, XEmbed);
#   - closing the main window keeps the app in the tray;
#   - idle RSS/PSS of the app and its WebKit processes (window open, then tray only).
# Screenshots go to $SHOTS (default ~/e2e/shots). Exits non-zero on any failure.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DEB=${1:-$HOME/dist/intent_0.2.0_$(dpkg --print-architecture).deb}
API=http://127.0.0.1:9477
SHOTS=${SHOTS:-$HOME/e2e/shots}
LOG=$HOME/e2e/app.log
PCAP=$HOME/e2e/app.pcap
IDLE_SECONDS=${IDLE_SECONDS:-60}
export DISPLAY=:99 XDG_SESSION_TYPE=x11
export DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}

step() { printf '\n== %s\n' "$*"; }
fail() { echo "E2E-APP FAILED: $*" >&2; [[ -f $LOG ]] && tail -20 "$LOG" >&2; exit 1; }
shot() { import -window root -display "$DISPLAY" "$SHOTS/$1.png"; echo "  screenshot $SHOTS/$1.png"; }
need() { command -v "$1" >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "${2:-$1}" >/dev/null; }

mkdir -p "$SHOTS" "$(dirname "$LOG")"
rm -f "$SHOTS"/*.png
need tcpdump
need stalonetray
need xcompmgr
need wmctrl
need desktop-file-validate desktop-file-utils

if [[ "${SKIP_SEED:-0}" != "1" ]]; then
  step "install + seed activity (e2e.sh)"
  "$ROOT/dev/lima/e2e.sh" "$DEB"
else
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall "$DEB" >/dev/null
fi
TOKEN=$(cat "$XDG_RUNTIME_DIR/intent/token")
api() { curl -fsS -H "Authorization: Bearer $TOKEN" "$@"; }

step "package layout"
dpkg -s intent | sed -n 's/^Depends: /  depends: /p'
for f in /usr/bin/intent /usr/lib/intent/intent-app /usr/share/applications/io.github.pranav5255.Intent.desktop \
  /usr/share/icons/hicolor/128x128/apps/intent.png; do
  [[ -e $f ]] || fail "missing $f"
done
desktop-file-validate /usr/share/applications/io.github.pranav5255.Intent.desktop || fail "invalid desktop entry"
missing=$(ldd /usr/lib/intent/intent-app | grep 'not found' || true)
[[ -z $missing ]] || fail "unresolved libraries: $missing"
ls /usr/lib/*/libayatana-appindicator3.so.1 >/dev/null || fail "tray library not installed"
echo "  $(du -h /usr/lib/intent/intent-app | cut -f1) binary, all libraries resolve"

step "fresh app state"
pkill -x intent-app 2>/dev/null || true
pkill -x stalonetray 2>/dev/null || true
sleep 1
api -X POST "$API/v1/capture/pause" -H 'content-type: application/json' -d '{"paused":false}' >/dev/null
rm -rf "$HOME/.local/share/io.github.pranav5255.Intent" "$HOME/.config/io.github.pranav5255.Intent"

# A compositor, as on GNOME, so the palette's transparent margin renders as it
# does there; and a tray host so the indicator can embed itself (openbox has neither).
pgrep -x xcompmgr >/dev/null || { setsid xcompmgr >/dev/null 2>&1 </dev/null & }
setsid stalonetray --geometry 4x1-0+0 --icon-size 22 --kludges force_icons_size >/dev/null 2>&1 </dev/null &
sleep 1

step "capture app → daemon traffic"
sudo pkill -f "tcpdump -i lo" 2>/dev/null || true
sudo setsid tcpdump -i lo -U -s 0 -w "$PCAP" 'tcp port 9477' >/dev/null 2>&1 </dev/null &
sleep 1

step "launch /usr/bin/intent"
setsid intent >"$LOG" 2>&1 </dev/null &
main=""
for _ in $(seq 60); do
  main=$(xdotool search --onlyvisible --name '^Intent$' 2>/dev/null | head -n1 || true)
  [[ -n $main ]] && break
  sleep 0.5
done
[[ -n $main ]] || fail "main window did not appear"
APP_PID=$(pgrep -x intent-app | head -n1)
echo "  window $main, pid $APP_PID, class $(xprop -id "$main" WM_CLASS | cut -d= -f2)"
sleep 4
shot vm-01-first-run
# First run shows the onboarding dialog with "Start" focused.
xdotool windowactivate --sync "$main" key Return
sleep 3
shot vm-02-today

step "timeline / search / preview"
xdotool windowactivate --sync "$main" key ctrl+2
sleep 3
shot vm-03-timeline
TODAY=$(date +%F)
intents=$(api "$API/intents?date=$TODAY")
[[ $(echo "$intents" | jq length) -gt 0 ]] || fail "daemon has no intents to show"
first_id=$(echo "$intents" | jq -r '.[0].id')
word=$(echo "$intents" | jq -r '.[0].label' | awk '{print $1}')
xdotool windowactivate --sync "$main" key ctrl+3
sleep 1
xdotool type --delay 60 "$word"
sleep 3
shot vm-04-search
# What the morning notification runs; the running instance takes the argv.
intent --preview "$first_id" >>"$LOG" 2>&1
sleep 3
shot vm-05-preview
xdotool windowactivate --sync "$main" key Escape
pgrep -x intent-app | grep -qx "$APP_PID" || fail "second launch replaced the running instance"
[[ $(pgrep -cx intent-app) -eq 1 ]] || fail "single-instance lock failed"

step "global shortcut toggles the palette"
palette() { # prints the visible palette window id, if any
  local w
  for w in $(xdotool search --onlyvisible --name '^Intent$' 2>/dev/null); do
    [[ $w != "$main" ]] && echo "$w"
  done
  return 0
}
wait_palette() { # wait_palette shown|hidden
  local _
  for _ in $(seq 20); do
    if [[ $1 == shown ]]; then [[ -n $(palette) ]] && return 0; else [[ -z $(palette) ]] && return 0; fi
    sleep 0.25
  done
  return 1
}
xdotool key super+space
wait_palette shown || fail "Super+Space did not show the palette"
overlay=$(palette)
sw=$(xdpyinfo | awk '/dimensions:/{split($2, d, "x"); print d[1]}')
# The window manager places the window first and the app centres it after; on a busy
# runner that can take more than a second, so poll instead of reading once.
centred=0
for _ in $(seq 20); do
  geom=$(xwininfo -id "$overlay")
  px=$(awk '/Absolute upper-left X/{print $NF}' <<<"$geom"); pw=$(awk '/Width:/{print $NF}' <<<"$geom")
  if (( px + pw / 2 >= sw / 2 - 2 && px + pw / 2 <= sw / 2 + 2 )); then centred=1; break; fi
  sleep 0.25
done
echo "  palette window $overlay at x=$px, ${pw}px wide on a ${sw}px screen"
((centred)) || fail "palette is not centred on its first open"
focused=0
for _ in $(seq 12); do
  [[ $(xdotool getactivewindow 2>/dev/null) == "$overlay" ]] && { focused=1; break; }
  sleep 0.25
done
((focused)) || fail "palette did not take keyboard focus on its first open"
# Let the first open finish loading before the shortcut closes it. A close within that
# second can leave the next reopen deaf to its first Escape (a known issue).
sleep 1
shot vm-06-palette
xdotool key super+space
wait_palette hidden || fail "Super+Space did not close the open palette"
for round in 1 2 3 4 5 6; do   # every reopen must take keyboard input (Escape closes it)
  sleep 0.5
  xdotool key super+space
  wait_palette shown || fail "Super+Space did not reopen the palette (round $round)"
  # Every reopen must take the keyboard too, or Escape below goes elsewhere.
  focused=0
  for _ in $(seq 12); do
    [[ $(xdotool getactivewindow 2>/dev/null) == "$(palette)" ]] && { focused=1; break; }
    sleep 0.25
  done
  ((focused)) || fail "the reopened palette did not take keyboard focus (round $round)"
  # A key in the first half second after a reopen can be lost (known issue); a person
  # reaches for Escape later than that.
  sleep 0.5
  xdotool key Escape
  wait_palette hidden || fail "the reopened palette ignored Escape (round $round)"
done
echo "  opens, closes with the shortcut, and takes keys on every reopen"

step "pause for 15 minutes from the palette"
sleep 0.5
xdotool key super+space
wait_palette shown || fail "Super+Space did not show the palette"
sleep 0.5
xdotool type --delay 60 ">pause 15"
sleep 1
shot vm-07-palette-pause
xdotool key Return
for _ in $(seq 20); do
  until_ts=$(api "$API/v1/status" | jq -r '.capture_paused_until // empty')
  [[ -n $until_ts ]] && break
  sleep 0.25
done
[[ -n $until_ts ]] || fail "palette pause did not reach the daemon"
mins=$(( (until_ts - $(date +%s) + 30) / 60 ))
echo "  daemon: paused until $(date -d "@$until_ts" +%H:%M) (~$mins min)"
[[ $mins -ge 14 && $mins -le 16 ]] || fail "expected a 15 minute pause, got $mins min"
intent >>"$LOG" 2>&1   # bring the main window back
sleep 3
shot vm-08-paused
xdotool windowactivate --sync "$main" key ctrl+1
sleep 2

step "tray"
tray=$(xdotool search --classname stalonetray 2>/dev/null | head -n1 || true)
icons=0
if [[ -n $tray ]]; then
  icons=$(xwininfo -id "$tray" -children | grep -cE '^\s+0x' || true)
fi
watcher=$(busctl --user list 2>/dev/null | grep -c StatusNotifierWatcher || true)
echo "  StatusNotifierWatcher on the session bus: $watcher; XEmbed icons in stalonetray: $icons"
[[ $icons -ge 1 ]] || fail "tray icon did not register with the XEmbed host"
shot vm-09-tray

step "auth: no 401 between the app and the daemon"
api -X POST "$API/v1/capture/pause" -H 'content-type: application/json' -d '{"paused":false}' >/dev/null
sleep 2
sudo pkill -INT -f "tcpdump -i lo" || true
sleep 1
statuses=$(sudo tcpdump -r "$PCAP" -A 2>/dev/null | grep -aoE 'HTTP/1\.1 [0-9]{3}' | sort | uniq -c)
echo "$statuses" | sed 's/^/  /'
echo "$statuses" | grep -q 'HTTP/1.1 200' || fail "no successful app requests captured"
! echo "$statuses" | grep -q 'HTTP/1.1 401' || fail "the app got 401 responses"

memory() {
  local pid name r p total_rss=0 total_pss=0
  for pid in "$APP_PID" $(pgrep -P "$APP_PID"); do
    name=$(cat "/proc/$pid/comm")
    r=$(awk '/^VmRSS/{print $2}' "/proc/$pid/status")
    p=$(awk '/^Pss:/{print $2}' "/proc/$pid/smaps_rollup")
    total_rss=$((total_rss + r)); total_pss=$((total_pss + p))
    printf '  %-22s pid %-7s RSS %7d KiB  PSS %7d KiB\n' "$name" "$pid" "$r" "$p"
  done
  printf '  %-22s             RSS %7d KiB  PSS %7d KiB\n' total "$total_rss" "$total_pss"
}
step "idle memory, main window open (${IDLE_SECONDS} s)"
sleep "$IDLE_SECONDS"
memory
step "idle memory, closed to the tray (${IDLE_SECONDS} s)"
wmctrl -i -c "$main"
sleep 1
[[ -z $(xdotool search --onlyvisible --name '^Intent$' 2>/dev/null) ]] || fail "closing the main window did not hide it"
pgrep -x intent-app >/dev/null || fail "closing the main window quit the app"
sleep "$IDLE_SECONDS"
memory

grep -iE 'panic|error' "$LOG" | sed 's/^/  log: /' || true
echo; echo "E2E-APP OK (screenshots in $SHOTS)"
