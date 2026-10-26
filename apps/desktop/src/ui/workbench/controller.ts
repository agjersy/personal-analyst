// Imperative access to the active session's dockview instance so sidebars,
// cards and the command palette can open tabs and toggle docks.
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import type { SessionId } from "../../ipc";
import { loadBuffer } from "../../state/actions";
import { useFiles } from "../../state/files";
import { notifyError } from "../../state/ui";

export const GROUPS = {
  editors: "editors",
  sidebar: "sidebar",
  chat: "chat",
  bottom: "bottom",
  panels: "panels",
} as const;

export type DockName = "sidebar" | "bottom" | "chat" | "panels";

let api: DockviewApi | null = null;
let session: SessionId | null = null;

export function setWorkbench(next: DockviewApi | null, s: SessionId | null): void {
  api = next;
  session = s;
}

export function workbenchApi(): DockviewApi | null {
  return api;
}

export function fileName(path: string): string {
  return path.split("/").pop() ?? path;
}

function ensureEditorsGroup(a: DockviewApi): string {
  if (!a.getGroup(GROUPS.editors)) a.addGroup({ id: GROUPS.editors, direction: "right" });
  return GROUPS.editors;
}

export async function openFile(path: string, line: number | null = null): Promise<void> {
  if (!api || !session) return;
  const id = `file:${path}`;
  try {
    await loadBuffer(session, path);
  } catch (e) {
    notifyError(e);
    return;
  }
  if (!api || !session) return;
  const existing = api.getPanel(id);
  if (existing) existing.api.setActive();
  else {
    const group = ensureEditorsGroup(api);
    api.addPanel({ id, component: "editor", title: fileName(path), params: { path }, position: { referenceGroup: group } });
  }
  useFiles.getState().setActiveFile(session, path);
  if (line !== null) useFiles.getState().requestReveal(session, path, line);
  else useFiles.getState().requestReveal(session, path, null);
}

export function openProposal(proposalId: string, title: string): void {
  if (!api) return;
  const id = `proposal:${proposalId}`;
  const existing = api.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return;
  }
  const group = ensureEditorsGroup(api);
  api.addPanel({ id, component: "proposal", title: `Review: ${title}`, params: { proposalId }, position: { referenceGroup: group } });
}

export function closePanel(id: string): void {
  api?.getPanel(id)?.api.close();
}

export function showPanel(id: string): void {
  if (!api) return;
  const panel = api.getPanel(id);
  if (!panel) return;
  const group = panel.group;
  if (!group.api.isVisible) group.api.setVisible(true);
  panel.api.setActive();
}

export function toggleDock(name: DockName): void {
  if (!api) return;
  const group = api.getGroup(GROUPS[name]);
  if (!group) return;
  group.api.setVisible(!group.api.isVisible);
}

export function isDockVisible(name: DockName): boolean {
  const group = api?.getGroup(GROUPS[name]);
  return group ? group.api.isVisible : false;
}

export function activePanel(): IDockviewPanel | undefined {
  return api?.activePanel;
}

export function closeActiveTab(): void {
  const panel = api?.activePanel;
  if (!panel) return;
  if (panel.api.component === "editor" || panel.api.component === "proposal") panel.api.close();
}

export function cycleTab(delta: 1 | -1): void {
  const panel = api?.activePanel;
  if (!panel) return;
  const panels = panel.group.panels;
  const idx = panels.indexOf(panel);
  if (idx < 0 || panels.length < 2) return;
  const next = panels[(idx + delta + panels.length) % panels.length];
  next?.api.setActive();
}

export function focusChatInput(): void {
  showPanel("chat");
  requestAnimationFrame(() => {
    const el = document.querySelector<HTMLTextAreaElement>("textarea[data-chat-input]");
    el?.focus();
  });
}

export function activeEditorPath(): string | null {
  const panel = api?.activePanel;
  if (panel && panel.api.component === "editor") return (panel.params as { path?: string } | undefined)?.path ?? null;
  return session ? (useFiles.getState().activeFile[session] ?? null) : null;
}
