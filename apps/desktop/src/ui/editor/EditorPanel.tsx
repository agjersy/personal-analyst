import { Compartment, EditorState } from "@codemirror/state";
import { setDiagnostics } from "@codemirror/lint";
import { EditorView } from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useEffect, useRef } from "react";
import { loadBuffer, saveBuffer } from "../../state/actions";
import { usePathDiagnostics } from "../../state/diagnostics";
import { useBuffer, useFiles } from "../../state/files";
import { useActiveSession } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { fileName } from "../workbench/controller";
import { registerEditorView } from "./registry";
import { baseExtensions, languageFor, toCmDiagnostics } from "./codemirror";

export function EditorPanel({ params, api }: IDockviewPanelProps<{ path: string }>) {
  const path = params.path;
  const session = useActiveSession();
  const buffer = useBuffer(session, path);
  const diagnostics = usePathDiagnostics(session, path);
  const reveal = useFiles((s) => (session ? s.reveal[session] : null));
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const languageRef = useRef(new Compartment());
  const appliedRevision = useRef(-1);

  useEffect(() => {
    if (session && !buffer) void loadBuffer(session, path).catch(notifyError);
  }, [session, path, buffer]);

  // Create the view once per panel.
  useEffect(() => {
    const host = hostRef.current;
    if (!host || !session) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: "",
        extensions: [
          baseExtensions(),
          languageRef.current.of(languageFor(path)),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) useFiles.getState().updateContent(session, path, u.state.doc.toString());
          }),
        ],
      }),
      parent: host,
    });
    viewRef.current = view;
    const unregister = registerEditorView(path, view);
    return () => {
      unregister();
      view.destroy();
      viewRef.current = null;
    };
  }, [session, path]);

  // Load text when the store delivers a new revision (open, reload).
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !buffer || buffer.loading) return;
    if (buffer.revision === appliedRevision.current) return;
    appliedRevision.current = buffer.revision;
    const current = view.state.doc.toString();
    if (current !== buffer.content) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: buffer.content } });
    }
  }, [buffer]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch(setDiagnostics(view.state, toCmDiagnostics(view.state.doc, diagnostics)));
  }, [diagnostics, buffer?.revision]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || !reveal || reveal.path !== path) return;
    const run = () => {
      const line = reveal.line !== null ? Math.max(1, Math.min(reveal.line, view.state.doc.lines)) : null;
      if (line !== null) {
        const l = view.state.doc.line(line);
        view.dispatch({ selection: { anchor: l.from }, effects: EditorView.scrollIntoView(l.from, { y: "center" }) });
      }
      view.focus();
    };
    const id = requestAnimationFrame(run);
    return () => cancelAnimationFrame(id);
  }, [reveal, path, buffer?.revision]);

  useEffect(() => {
    api.setTitle(`${fileName(path)}${buffer?.dirty ? " •" : ""}`);
  }, [api, path, buffer?.dirty]);

  const reload = () => session && void loadBuffer(session, path, true).catch(notifyError);
  const keep = () => session && useFiles.getState().clearExternal(session, path);
  const overwrite = () => session && void saveBuffer(session, path, true);

  return (
    <div className="editor" data-testid="editor" data-path={path}>
      {buffer?.error ? <div className="editor-banner error">{buffer.error}</div> : null}
      {buffer?.external === "changed" ? (
        <div className="editor-banner" role="status">
          <span className="grow">{buffer.dirty ? "This file changed on disk while you have unsaved edits." : "This file changed on disk."}</span>
          <button type="button" className="small" onClick={reload}>
            Reload
          </button>
          {buffer.dirty ? (
            <button type="button" className="small" onClick={overwrite}>
              Overwrite
            </button>
          ) : null}
          <button type="button" className="small ghost" onClick={keep}>
            Dismiss
          </button>
        </div>
      ) : null}
      {buffer?.external === "deleted" ? (
        <div className="editor-banner error" role="status">
          <span className="grow">This file was deleted on disk.</span>
          <button type="button" className="small" onClick={overwrite}>
            Save again
          </button>
          <button type="button" className="small ghost" onClick={() => api.close()}>
            Close
          </button>
        </div>
      ) : null}
      <div className="editor-host" ref={hostRef} />
    </div>
  );
}
