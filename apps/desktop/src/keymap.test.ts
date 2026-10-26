import { describe, expect, it } from "vitest";
import { KEYMAP, bindingFor, formatKey, matchesKey, parseKey, resolveKey } from "./keymap";

const ev = (key: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) => ({
  key,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

describe("keymap table", () => {
  it("has unique command ids per context and parseable keys", () => {
    const seen = new Set<string>();
    for (const b of KEYMAP) {
      const id = `${b.context}:${b.command}`;
      expect(seen.has(id)).toBe(false);
      seen.add(id);
      expect(() => parseKey(b.key)).not.toThrow();
    }
  });

  it("does not bind the same chord twice within one context", () => {
    const byContext = new Map<string, Set<string>>();
    for (const b of KEYMAP) {
      const set = byContext.get(b.context) ?? new Set();
      expect(set.has(b.key)).toBe(false);
      set.add(b.key);
      byContext.set(b.context, set);
    }
  });

  it("covers the Zed subset", () => {
    const expected: Record<string, string> = {
      file_finder: "mod-p",
      command_palette: "mod-shift-p",
      save: "mod-s",
      toggle_left_dock: "mod-b",
      toggle_bottom_dock: "mod-j",
      toggle_right_dock: "mod-r",
      focus_chat: "mod-l",
      new_terminal: "ctrl-`",
      close_tab: "mod-w",
      next_tab: "ctrl-tab",
      prev_tab: "ctrl-shift-tab",
      go_to_line: "ctrl-g",
      run_current_script: "mod-enter",
    };
    for (const [cmd, key] of Object.entries(expected)) expect(bindingFor(cmd)?.key).toBe(key);
  });
});

describe("matching", () => {
  it("maps mod to Cmd on macOS and Ctrl elsewhere", () => {
    expect(matchesKey(ev("p", { metaKey: true }), "mod-p", true)).toBe(true);
    expect(matchesKey(ev("p", { ctrlKey: true }), "mod-p", true)).toBe(false);
    expect(matchesKey(ev("p", { ctrlKey: true }), "mod-p", false)).toBe(true);
    expect(matchesKey(ev("p", { metaKey: true }), "mod-p", false)).toBe(false);
  });

  it("keeps ctrl as Control on both platforms", () => {
    expect(matchesKey(ev("`", { ctrlKey: true }), "ctrl-`", true)).toBe(true);
    expect(matchesKey(ev("`", { ctrlKey: true }), "ctrl-`", false)).toBe(true);
    expect(matchesKey(ev("Tab", { ctrlKey: true, shiftKey: true }), "ctrl-shift-tab", false)).toBe(true);
    expect(matchesKey(ev("Tab", { ctrlKey: true }), "ctrl-shift-tab", false)).toBe(false);
  });

  it("prefers context bindings over global ones", () => {
    expect(resolveKey(ev("Enter", { ctrlKey: true }), "editor", false)).toBe("run_current_script");
    expect(resolveKey(ev("Enter", { ctrlKey: true }), "chat", false)).toBe("chat_send");
    expect(resolveKey(ev("Enter", { ctrlKey: true }), "global", false)).toBeNull();
    expect(resolveKey(ev("s", { ctrlKey: true }), "editor", false)).toBe("save");
    expect(resolveKey(ev("P", { ctrlKey: true, shiftKey: true }), "global", false)).toBe("command_palette");
  });

  it("formats bindings per platform", () => {
    expect(formatKey("mod-shift-p", true)).toBe("⇧⌘P");
    expect(formatKey("mod-shift-p", false)).toBe("Ctrl+Shift+P");
    expect(formatKey("ctrl-`", true)).toBe("⌃`");
    expect(formatKey("mod-enter", false)).toBe("Ctrl+Enter");
  });
});
