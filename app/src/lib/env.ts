// Runtime environment: Tauri vs plain browser, mock mode, platform conventions.

export const MOCK = import.meta.env.VITE_MOCK === '1' || import.meta.env.VITE_MOCK === 'true';

export const IN_TAURI = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** Fixed "now" for mock mode so fixtures and screenshots are deterministic. */
const MOCK_NOW_MS = new Date(2026, 10, 14, 15, 42, 0).getTime();

export function nowMs(): number {
  return MOCK ? MOCK_NOW_MS : Date.now();
}

export function nowSec(): number {
  return Math.floor(nowMs() / 1000);
}

const platform = typeof navigator !== 'undefined' ? navigator.userAgent : '';
export const IS_MAC = /Mac|iPhone|iPad/.test(platform);

/** Modifier glyph for keycaps: ⌘ on macOS, Ctrl elsewhere (GNOME convention). */
export const MOD = IS_MAC ? '⌘' : 'Ctrl';

/** The command that starts the daemon: a launchd agent on macOS, a systemd unit on Linux. */
export const START_DAEMON = IS_MAC ? 'intentctl enable' : 'systemctl --user start intentd';

/** True when the platform's primary modifier is held. */
export function modHeld(event: { metaKey: boolean; ctrlKey: boolean }): boolean {
  return IS_MAC ? event.metaKey : event.ctrlKey;
}

export type WindowKind = 'overlay' | 'main';

/** Which surface this webview renders. Tauri windows load `#/overlay` or `#/main`. */
export function windowKind(): WindowKind {
  if (IN_TAURI) {
    const internals = (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } })
      .__TAURI_INTERNALS__;
    const label = internals?.metadata?.currentWindow?.label;
    if (label === 'overlay' || label === 'main') return label;
  }
  return window.location.hash.startsWith('#/overlay') ? 'overlay' : 'main';
}
