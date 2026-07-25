import type { CommandMode } from '../types';

export interface ParsedCommand {
  mode: CommandMode;
  query: string;
}

export function parseCommand(value: string): ParsedCommand {
  const trimmed = value.trimStart();
  if (trimmed.startsWith('?')) return { mode: 'copilot', query: trimmed.slice(1).trim() };
  if (trimmed.startsWith('!')) return { mode: 'restore', query: trimmed.slice(1).trim() };
  return { mode: 'search', query: trimmed.startsWith('/') ? trimmed.slice(1).trim() : trimmed };
}

export function modeLabel(mode: CommandMode): string {
  if (mode === 'copilot') return 'Copilot';
  if (mode === 'restore') return 'Restore';
  return 'Search';
}
