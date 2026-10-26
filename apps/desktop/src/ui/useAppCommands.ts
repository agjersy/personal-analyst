// Registers the command table and the global key handler.
import { useEffect } from "react";
import { registerCommands, runCommand } from "../commands";
import { resolveKey } from "../keymap";
import type { KeyContext } from "../keymap";
import { openTerminal, refreshIndex, runScript, saveBuffer } from "../state/actions";
import { useFiles } from "../state/files";
import { useSessions } from "../state/sessions";
import { useUi } from "../state/ui";
import { activeEditorPath, closeActiveTab, cycleTab, focusChatInput, showPanel, toggleDock } from "./workbench/controller";

function contextOf(target: EventTarget | null): KeyContext {
  const el = target instanceof Element ? target : null;
  if (el?.closest(".cm-editor")) return "editor";
  if (el?.closest("[data-chat-input]")) return "chat";
  return "global";
}

export function useAppCommands(): void {
  useEffect(() => {
    const session = () => useSessions.getState().active;
    const unregister = registerCommands([
      { id: "file_finder", title: "Go to file", category: "Navigate", run: () => useUi.getState().setPalette("files") },
      { id: "command_palette", title: "Command palette", category: "Navigate", run: () => useUi.getState().setPalette("commands") },
      { id: "go_to_line", title: "Go to line", category: "Navigate", run: () => useUi.getState().setPalette("goto") },
      {
        id: "save",
        title: "Save file",
        category: "File",
        run: () => {
          const s = session();
          const path = activeEditorPath();
          if (s && path) void saveBuffer(s, path);
        },
      },
      { id: "close_tab", title: "Close tab", category: "File", run: closeActiveTab },
      { id: "next_tab", title: "Next tab", category: "Navigate", run: () => cycleTab(1) },
      { id: "prev_tab", title: "Previous tab", category: "Navigate", run: () => cycleTab(-1) },
      { id: "toggle_left_dock", title: "Toggle left dock", category: "View", run: () => toggleDock("sidebar") },
      { id: "toggle_bottom_dock", title: "Toggle bottom dock", category: "View", run: () => toggleDock("bottom") },
      { id: "toggle_right_dock", title: "Toggle right dock", category: "View", run: () => toggleDock("chat") },
      { id: "toggle_panels_dock", title: "Toggle figures dock", category: "View", run: () => toggleDock("panels") },
      { id: "focus_chat", title: "Focus agent chat", category: "Agent", run: focusChatInput },
      {
        id: "chat_send",
        title: "Send chat message",
        category: "Agent",
        hidden: true,
        run: () => {
          const el = document.querySelector<HTMLTextAreaElement>("textarea[data-chat-input]");
          el?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
        },
      },
      {
        id: "new_terminal",
        title: "New terminal",
        category: "Terminal",
        run: () => {
          const s = session();
          if (s) {
            showPanel("terminal");
            void openTerminal(s);
          }
        },
      },
      {
        id: "run_current_script",
        title: "Run current script",
        category: "Run",
        run: () => {
          const s = session();
          const path = activeEditorPath();
          if (!s || !path || !path.endsWith(".py")) return;
          void (async () => {
            const b = useFiles.getState().buffers[s]?.[path];
            if (b?.dirty) await saveBuffer(s, path);
            const id = await runScript(s, path, []);
            if (id) showPanel("runoutput");
          })();
        },
      },
      {
        id: "refresh_index",
        title: "Refresh script and library index",
        category: "Run",
        run: () => {
          const s = session();
          if (s) void refreshIndex(s);
        },
      },
      { id: "show_figures", title: "Show figures", category: "View", run: () => showPanel("figures") },
      { id: "show_equations", title: "Show equations", category: "View", run: () => showPanel("equations") },
      { id: "show_packages", title: "Show packages", category: "View", run: () => showPanel("packages") },
      { id: "show_diagnostics", title: "Show diagnostics", category: "View", run: () => showPanel("diagnostics") },
      { id: "show_run_output", title: "Show run output", category: "View", run: () => showPanel("runoutput") },
      { id: "new_session", title: "New session", category: "Session", run: () => useUi.getState().setNewSessionOpen(true) },
      { id: "settings", title: "Settings and policy", category: "Session", run: () => useUi.getState().setSettingsOpen(true) },
      { id: "theme_light", title: "Theme: light", category: "View", run: () => useUi.getState().setTheme("light") },
      { id: "theme_dark", title: "Theme: dark", category: "View", run: () => useUi.getState().setTheme("dark") },
      { id: "theme_system", title: "Theme: system", category: "View", run: () => useUi.getState().setTheme("system") },
    ]);

    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      const ctx = contextOf(e.target);
      const command = resolveKey(e, ctx);
      if (!command) return;
      // The chat input handles its own send binding.
      if (command === "chat_send") return;
      e.preventDefault();
      e.stopPropagation();
      if (command === "command_palette" && useUi.getState().palette === "commands") {
        useUi.getState().setPalette(null);
        return;
      }
      runCommand(command);
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      unregister();
      document.removeEventListener("keydown", onKey, true);
    };
  }, []);
}
