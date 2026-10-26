// Live EditorView instances by path, for commands that act on the editor
// (selection attachment, go to line) without going through React.
import type { EditorView } from "@codemirror/view";
import type { Attachment } from "../../ipc";

const views = new Map<string, EditorView>();

export function registerEditorView(path: string, view: EditorView): () => void {
  views.set(path, view);
  return () => {
    if (views.get(path) === view) views.delete(path);
  };
}

export function getEditorView(path: string): EditorView | undefined {
  return views.get(path);
}

export function editorSelection(path: string): Attachment | null {
  const view = views.get(path);
  if (!view) return null;
  const range = view.state.selection.main;
  if (range.empty) return null;
  const doc = view.state.doc;
  return {
    kind: "selection",
    path,
    start_line: doc.lineAt(range.from).number,
    end_line: doc.lineAt(range.to).number,
    text: doc.sliceString(range.from, range.to),
  };
}

export function goToLine(path: string, line: number): void {
  const view = views.get(path);
  if (!view) return;
  const n = Math.max(1, Math.min(line, view.state.doc.lines));
  const l = view.state.doc.line(n);
  view.dispatch({ selection: { anchor: l.from }, scrollIntoView: true });
  view.focus();
}
