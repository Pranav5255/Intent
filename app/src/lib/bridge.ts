// Thin wrapper over Tauri commands/events. In a plain browser (mock mode or
// design work) every call degrades to a harmless no-op so the UI still renders.

import { IN_TAURI, MOCK } from './env';

type Unlisten = () => void;

export async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!IN_TAURI) throw new Error(`Not running inside the Intent app (${name}).`);
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(name, args);
}

export async function listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
  if (!IN_TAURI) return () => undefined;
  const { listen: tauriListen } = await import('@tauri-apps/api/event');
  return tauriListen<T>(event, (e) => handler(e.payload));
}

export async function emit<T>(event: string, payload: T): Promise<void> {
  if (!IN_TAURI) return;
  const { emit: tauriEmit } = await import('@tauri-apps/api/event');
  await tauriEmit(event, payload);
}

export async function hideOverlay(): Promise<void> {
  if (!IN_TAURI) return;
  await command('hide_overlay').catch(() => undefined);
}

export type MainRoute = 'today' | 'timeline' | 'search' | 'settings' | `intent:${string}` | `settings:${string}`;

export async function openMain(route?: MainRoute): Promise<void> {
  if (!IN_TAURI) {
    // Browser preview: navigate in place so flows can still be exercised.
    window.location.hash = `#/main/${route ?? 'today'}`;
    return;
  }
  await command('open_main', { route: route ?? null });
}

export async function takePendingRoute(): Promise<string | null> {
  if (!IN_TAURI) return null;
  return command<string | null>('take_pending_route').catch(() => null);
}

export async function getAccent(): Promise<string | null> {
  if (!IN_TAURI) return null;
  return command<string | null>('get_accent').catch(() => null);
}

export async function setShortcut(accelerator: string): Promise<void> {
  if (!IN_TAURI) {
    if (MOCK) return;
    throw new Error('Shortcuts can only be changed in the installed app.');
  }
  await command('set_shortcut', { accelerator });
}

export async function getShortcut(): Promise<string> {
  if (!IN_TAURI) return 'Super+Space';
  return command<string>('get_shortcut').catch(() => 'Super+Space');
}

export async function getAutostart(): Promise<boolean> {
  if (!IN_TAURI) return true;
  return command<boolean>('get_autostart').catch(() => false);
}

export async function setAutostart(enabled: boolean): Promise<void> {
  if (!IN_TAURI) return;
  await command('set_autostart', { enabled });
}

export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Fallback for webviews without clipboard permission.
    const area = document.createElement('textarea');
    area.value = text;
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
}

export async function appVersion(): Promise<string> {
  if (!IN_TAURI) return '2.0.0';
  const { getVersion } = await import('@tauri-apps/api/app');
  return getVersion();
}
