// Command registry: every user action has an id so the palette, keymap and
// menus share one table.
import { bindingFor, formatKey } from "./keymap";

export interface CommandDef {
  id: string;
  title: string;
  category?: string;
  run: () => void | Promise<void>;
  /** Hidden from the palette (e.g. context-only commands). */
  hidden?: boolean;
}

const registry = new Map<string, CommandDef>();
const listeners = new Set<() => void>();

export function registerCommands(defs: CommandDef[]): () => void {
  for (const d of defs) registry.set(d.id, d);
  for (const fn of listeners) fn();
  return () => {
    for (const d of defs) if (registry.get(d.id) === d) registry.delete(d.id);
    for (const fn of listeners) fn();
  };
}

export function onCommandsChanged(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function listCommands(): CommandDef[] {
  return [...registry.values()].filter((c) => !c.hidden).sort((a, b) => a.title.localeCompare(b.title));
}

export function runCommand(id: string): boolean {
  const c = registry.get(id);
  if (!c) return false;
  void c.run();
  return true;
}

export function commandShortcut(id: string): string | null {
  const b = bindingFor(id);
  return b ? formatKey(b.key) : null;
}
