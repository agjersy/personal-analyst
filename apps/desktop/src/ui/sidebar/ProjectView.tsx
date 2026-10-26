import { useCallback, useEffect, useState } from "react";
import { backend } from "../../ipc";
import type { FileEntry } from "../../ipc";
import { loadDir } from "../../state/actions";
import { useDiagnostics } from "../../state/diagnostics";
import { dirOf, useFiles } from "../../state/files";
import { useActiveSession } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { ConfirmDialog, InputDialog } from "../common/Dialog";
import { ContextMenu } from "../common/ContextMenu";
import type { MenuItem } from "../common/ContextMenu";
import { closePanel, openFile } from "../workbench/controller";

type Menu = { x: number; y: number; entry: FileEntry | null };
type Prompt =
  | { kind: "new-file"; dir: string }
  | { kind: "new-dir"; dir: string }
  | { kind: "rename"; entry: FileEntry }
  | { kind: "delete"; entry: FileEntry }
  | null;

export function ProjectView() {
  const session = useActiveSession();
  const trees = useFiles((s) => (session ? s.trees[session] : undefined));
  const expanded = useFiles((s) => (session ? s.expanded[session] : undefined));
  const activeFile = useFiles((s) => (session ? s.activeFile[session] : null));
  const diagnostics = useDiagnostics((s) => (session ? s.byPath[session] : undefined));
  const [menu, setMenu] = useState<Menu | null>(null);
  const [prompt, setPrompt] = useState<Prompt>(null);

  useEffect(() => {
    if (session && !trees?.[""]) void loadDir(session, "").catch(notifyError);
  }, [session, trees]);

  const toggle = useCallback(
    (entry: FileEntry) => {
      if (!session) return;
      const open = !(expanded?.[entry.path] ?? false);
      useFiles.getState().setExpanded(session, entry.path, open);
      if (open && !trees?.[entry.path]) void loadDir(session, entry.path).catch(notifyError);
    },
    [session, expanded, trees],
  );

  const onContext = (e: React.MouseEvent, entry: FileEntry | null) => {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY, entry });
  };

  const items: MenuItem[] = (() => {
    const entry = menu?.entry ?? null;
    const dir = entry ? (entry.kind === "dir" ? entry.path : dirOf(entry.path)) : "";
    const list: MenuItem[] = [
      { label: "New file…", onSelect: () => setPrompt({ kind: "new-file", dir }) },
      { label: "New folder…", onSelect: () => setPrompt({ kind: "new-dir", dir }) },
    ];
    if (entry) {
      list.push(
        { label: "Rename…", onSelect: () => setPrompt({ kind: "rename", entry }) },
        { label: "Delete", danger: true, onSelect: () => setPrompt({ kind: "delete", entry }) },
      );
    }
    return list;
  })();

  const run = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      notifyError(e);
    } finally {
      setPrompt(null);
    }
  };

  const renderEntries = (entries: FileEntry[], depth: number): React.ReactNode =>
    entries.map((entry) => {
      const isOpen = entry.kind === "dir" && (expanded?.[entry.path] ?? false);
      const children = entry.kind === "dir" ? (trees?.[entry.path] ?? entry.children ?? null) : null;
      const diag = diagnostics?.[entry.path];
      const worst = diag?.some((d) => d.severity === "error") ? "error" : diag?.length ? "warning" : null;
      return (
        <div key={entry.path} role="treeitem" aria-expanded={entry.kind === "dir" ? isOpen : undefined}>
          <button
            type="button"
            className={`tree-row${activeFile === entry.path ? " selected" : ""}`}
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => (entry.kind === "dir" ? toggle(entry) : void openFile(entry.path))}
            onContextMenu={(e) => onContext(e, entry)}
            data-path={entry.path}
          >
            <span className="chevron">{entry.kind === "dir" ? (isOpen ? "▾" : "▸") : ""}</span>
            <span className="truncate">{entry.name}</span>
            {worst ? <span className={`dot diag-dot ${worst === "error" ? "error" : "starting"}`} title={`${diag?.length} diagnostics`} /> : null}
          </button>
          {isOpen && children ? renderEntries(children, depth + 1) : null}
        </div>
      );
    });

  if (!session) return <div className="empty">No session</div>;
  const root = trees?.[""];
  return (
    <div className="panel" onContextMenu={(e) => e.target === e.currentTarget && onContext(e, null)}>
      <div className="panel-toolbar">
        <span className="grow small muted truncate">Files</span>
        <button type="button" className="small" onClick={() => setPrompt({ kind: "new-file", dir: "" })} aria-label="New file">
          + File
        </button>
        <button type="button" className="small" onClick={() => session && void loadDir(session, "").catch(notifyError)} aria-label="Refresh tree">
          ↻
        </button>
      </div>
      <div className="panel-body tree" role="tree" onContextMenu={(e) => e.target === e.currentTarget && onContext(e, null)}>
        {root ? renderEntries(root, 0) : <div className="empty">Loading…</div>}
      </div>
      {menu ? <ContextMenu x={menu.x} y={menu.y} items={items} onClose={() => setMenu(null)} /> : null}
      {prompt?.kind === "new-file" || prompt?.kind === "new-dir" ? (
        <InputDialog
          title={prompt.kind === "new-file" ? "New file" : "New folder"}
          label={`Name (inside ${prompt.dir || "repo root"})`}
          confirmLabel="Create"
          onClose={() => setPrompt(null)}
          onSubmit={(name) => {
            const path = prompt.dir ? `${prompt.dir}/${name}` : name;
            void run(async () => {
              await backend().invoke("fs_create", { session, path, is_dir: prompt.kind === "new-dir" });
              await loadDir(session, prompt.dir);
              if (prompt.dir) useFiles.getState().setExpanded(session, prompt.dir, true);
              if (prompt.kind === "new-file") await openFile(path);
            });
          }}
        />
      ) : null}
      {prompt?.kind === "rename" ? (
        <InputDialog
          title="Rename"
          label="New name"
          initial={prompt.entry.name}
          confirmLabel="Rename"
          onClose={() => setPrompt(null)}
          onSubmit={(name) => {
            const dir = dirOf(prompt.entry.path);
            const to = dir ? `${dir}/${name}` : name;
            void run(async () => {
              await backend().invoke("fs_rename", { session, from: prompt.entry.path, to });
              useFiles.getState().renameBuffer(session, prompt.entry.path, to);
              closePanel(`file:${prompt.entry.path}`);
              await loadDir(session, dir);
            });
          }}
        />
      ) : null}
      {prompt?.kind === "delete" ? (
        <ConfirmDialog
          title="Delete"
          message={`Delete ${prompt.entry.path}? It is moved to the trash when the platform supports it.`}
          confirmLabel="Delete"
          danger
          onClose={() => setPrompt(null)}
          onConfirm={() =>
            void run(async () => {
              await backend().invoke("fs_delete", { session, path: prompt.entry.path });
              closePanel(`file:${prompt.entry.path}`);
              useFiles.getState().closeBuffer(session, prompt.entry.path);
              await loadDir(session, dirOf(prompt.entry.path));
            })
          }
        />
      ) : null}
    </div>
  );
}
