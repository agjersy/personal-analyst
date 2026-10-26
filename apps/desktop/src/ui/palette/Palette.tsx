import { useEffect, useMemo, useRef, useState } from "react";
import { backend } from "../../ipc";
import type { FileEntry } from "../../ipc";
import { commandShortcut, listCommands, onCommandsChanged, runCommand } from "../../commands";
import { useActiveSession } from "../../state/sessions";
import { useUi } from "../../state/ui";
import type { PaletteMode } from "../../state/ui";
import { goToLine } from "../editor/registry";
import { activeEditorPath, openFile } from "../workbench/controller";

interface Item {
  id: string;
  label: string;
  detail?: string;
  shortcut?: string | null;
  run: () => void;
}

function flatten(entries: FileEntry[], out: string[] = []): string[] {
  for (const e of entries) {
    if (e.kind === "file") out.push(e.path);
    else if (e.children) flatten(e.children, out);
  }
  return out;
}

function fuzzyScore(query: string, text: string): number {
  if (!query) return 1;
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (t.includes(q)) return 100 - t.indexOf(q);
  let qi = 0;
  for (let i = 0; i < t.length && qi < q.length; i++) if (t[i] === q[qi]) qi++;
  return qi === q.length ? 10 : 0;
}

export function Palette() {
  const mode = useUi((s) => s.palette);
  if (!mode) return null;
  return <PaletteBody key={mode} mode={mode} />;
}

function PaletteBody({ mode }: { mode: Exclude<PaletteMode, null> }) {
  const session = useActiveSession();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [files, setFiles] = useState<string[]>([]);
  const [, bump] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const close = () => useUi.getState().setPalette(null);

  useEffect(() => onCommandsChanged(() => bump((n) => n + 1)), []);

  useEffect(() => {
    requestAnimationFrame(() => inputRef.current?.focus());
    if (mode === "files" && session) {
      backend()
        .invoke("fs_tree", { session, depth: 12 })
        .then((tree) => setFiles(flatten(tree).filter((p) => !p.startsWith("out/"))))
        .catch(() => setFiles([]));
    }
  }, [mode, session]);

  const items: Item[] = useMemo(() => {
    if (mode === "commands") {
      return listCommands()
        .map((c) => ({ id: c.id, label: c.title, detail: c.category, shortcut: commandShortcut(c.id), run: () => runCommand(c.id) }))
        .map((it) => ({ it, score: fuzzyScore(query, it.label) }))
        .filter((x) => x.score > 0)
        .sort((a, b) => b.score - a.score)
        .map((x) => x.it);
    }
    if (mode === "files") {
      return files
        .map((p) => ({ p, score: fuzzyScore(query, p) }))
        .filter((x) => x.score > 0)
        .sort((a, b) => b.score - a.score)
        .slice(0, 60)
        .map(({ p }) => ({ id: p, label: p.split("/").pop() ?? p, detail: p, run: () => void openFile(p) }));
    }
    if (mode === "goto") {
      const path = activeEditorPath();
      const n = Number.parseInt(query, 10);
      return [
        {
          id: "goto",
          label: path ? (Number.isFinite(n) ? `Go to line ${n} in ${path.split("/").pop()}` : "Type a line number") : "No active editor",
          run: () => {
            if (path && Number.isFinite(n)) goToLine(path, n);
          },
        },
      ];
    }
    return [];
  }, [mode, query, files]);

  const run = (it: Item | undefined) => {
    if (!it) return;
    close();
    it.run();
  };
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="palette" role="dialog" aria-label={mode === "commands" ? "Command palette" : mode === "files" ? "File finder" : "Go to line"} data-testid="palette">
        <input
          ref={inputRef}
          type="text"
          value={query}
          placeholder={mode === "commands" ? "Run a command…" : mode === "files" ? "Open a file…" : "Line number"}
          aria-label="Palette query"
          onChange={(e) => {
            setQuery(e.target.value);
            setIndex(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setIndex((i) => Math.min(items.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              run(items[index]);
            } else if (e.key === "Escape") {
              close();
            }
          }}
        />
        <div className="palette-list" role="listbox">
          {items.length === 0 ? <div className="empty">No matches</div> : null}
          {items.map((it, i) => (
            <button key={it.id} type="button" role="option" aria-selected={i === index} className={`palette-item${i === index ? " active" : ""}`} onMouseEnter={() => setIndex(i)} onClick={() => run(it)}>
              <span>{it.label}</span>
              {it.detail ? <span className="muted small truncate">{it.detail}</span> : null}
              {it.shortcut ? <span className="kbd">{it.shortcut}</span> : null}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
