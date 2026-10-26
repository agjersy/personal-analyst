// Zed-style keymap (subset). `mod` is Cmd on macOS and Ctrl elsewhere; `ctrl`
// is Control everywhere. Keys are lower-case; `+` is not used so the strings
// stay unambiguous for keys like "-".

export type KeyContext = "global" | "editor" | "chat";

export interface KeyBinding {
  command: string;
  key: string;
  context: KeyContext;
}

export const KEYMAP: readonly KeyBinding[] = [
  { command: "file_finder", key: "mod-p", context: "global" },
  { command: "command_palette", key: "mod-shift-p", context: "global" },
  { command: "save", key: "mod-s", context: "global" },
  { command: "toggle_left_dock", key: "mod-b", context: "global" },
  { command: "toggle_bottom_dock", key: "mod-j", context: "global" },
  { command: "toggle_right_dock", key: "mod-r", context: "global" },
  { command: "focus_chat", key: "mod-l", context: "global" },
  { command: "new_terminal", key: "ctrl-`", context: "global" },
  { command: "close_tab", key: "mod-w", context: "global" },
  { command: "next_tab", key: "ctrl-tab", context: "global" },
  { command: "prev_tab", key: "ctrl-shift-tab", context: "global" },
  { command: "go_to_line", key: "ctrl-g", context: "global" },
  { command: "run_current_script", key: "mod-enter", context: "editor" },
  { command: "chat_send", key: "mod-enter", context: "chat" },
];

export function isMac(platform: string = typeof navigator === "undefined" ? "" : navigator.platform): boolean {
  return /mac|iphone|ipad/i.test(platform);
}

export interface ParsedKey {
  mod: boolean;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

export function parseKey(spec: string): ParsedKey {
  const parts = spec.toLowerCase().split("-");
  const key = parts.pop() ?? "";
  const p: ParsedKey = { mod: false, ctrl: false, shift: false, alt: false, key };
  for (const m of parts) {
    if (m === "mod") p.mod = true;
    else if (m === "ctrl") p.ctrl = true;
    else if (m === "shift") p.shift = true;
    else if (m === "alt") p.alt = true;
    else throw new Error(`unknown modifier ${m} in ${spec}`);
  }
  return p;
}

interface KeyLike {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

function normalizeEventKey(key: string): string {
  if (key === " ") return "space";
  if (key === "Dead") return "`";
  return key.length === 1 ? key.toLowerCase() : key.toLowerCase();
}

export function matchesKey(e: KeyLike, spec: string, mac: boolean = isMac()): boolean {
  const p = parseKey(spec);
  const wantMeta = mac ? p.mod : false;
  const wantCtrl = mac ? p.ctrl : p.ctrl || p.mod;
  if (e.metaKey !== wantMeta) return false;
  if (e.ctrlKey !== wantCtrl) return false;
  if (e.shiftKey !== p.shift) return false;
  if (e.altKey !== p.alt) return false;
  return normalizeEventKey(e.key) === p.key;
}

/** Resolves a key event to a command id for the given focus context. */
export function resolveKey(e: KeyLike, context: KeyContext, mac: boolean = isMac()): string | null {
  const candidates = KEYMAP.filter((b) => b.context === context || b.context === "global");
  // Context-specific bindings win over global ones.
  candidates.sort((a, b) => (a.context === "global" ? 1 : 0) - (b.context === "global" ? 1 : 0));
  for (const b of candidates) if (matchesKey(e, b.key, mac)) return b.command;
  return null;
}

export function bindingFor(command: string): KeyBinding | undefined {
  return KEYMAP.find((b) => b.command === command);
}

const MAC_SYMBOLS: Record<string, string> = { mod: "⌘", ctrl: "⌃", shift: "⇧", alt: "⌥" };
const KEY_NAMES: Record<string, string> = { enter: "Enter", tab: "Tab", "`": "`", escape: "Esc", space: "Space" };

export function formatKey(spec: string, mac: boolean = isMac()): string {
  const p = parseKey(spec);
  const keyName = KEY_NAMES[p.key] ?? p.key.toUpperCase();
  if (mac) {
    return `${p.ctrl ? MAC_SYMBOLS.ctrl : ""}${p.alt ? MAC_SYMBOLS.alt : ""}${p.shift ? MAC_SYMBOLS.shift : ""}${p.mod ? MAC_SYMBOLS.mod : ""}${keyName}`;
  }
  const parts: string[] = [];
  if (p.ctrl || p.mod) parts.push("Ctrl");
  if (p.alt) parts.push("Alt");
  if (p.shift) parts.push("Shift");
  parts.push(keyName);
  return parts.join("+");
}
