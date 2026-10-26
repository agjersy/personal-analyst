import { getChunks, rejectChunk, unifiedMergeView } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { syntaxHighlighting } from "@codemirror/language";
import { useEffect, useRef } from "react";
import { highlightStyle, languageFor } from "./codemirror";

export interface MergeEditorHandle {
  getDoc(): string;
  rejectAll(): void;
  acceptAll(): void;
  chunkCount(): number;
}

interface MergeEditorProps {
  path: string;
  original: string;
  proposed: string;
  readOnly: boolean;
  onHandle: (handle: MergeEditorHandle | null) => void;
  onChunksChange: (remaining: number) => void;
}

export default function MergeEditor({ path, original, proposed, readOnly, onHandle, onChunksChange }: MergeEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const view = new EditorView({
      state: EditorState.create({
        doc: proposed,
        extensions: [
          lineNumbers(),
          languageFor(path),
          syntaxHighlighting(highlightStyle, { fallback: true }),
          unifiedMergeView({ original, mergeControls: !readOnly, highlightChanges: true, gutter: true, collapseUnchanged: { margin: 3, minSize: 6 } }),
          EditorView.editable.of(!readOnly),
          EditorState.readOnly.of(readOnly),
          EditorView.lineWrapping,
          EditorView.updateListener.of((u) => {
            if (u.docChanged || u.transactions.some((t) => t.effects.length > 0)) {
              onChunksChange(getChunks(u.state)?.chunks.length ?? 0);
            }
          }),
        ],
      }),
      parent: host,
    });
    onChunksChange(getChunks(view.state)?.chunks.length ?? 0);
    onHandle({
      getDoc: () => view.state.doc.toString(),
      rejectAll: () => {
        // Reject from the end so earlier positions stay valid.
        for (;;) {
          const chunks = getChunks(view.state)?.chunks ?? [];
          const last = chunks[chunks.length - 1];
          if (!last) break;
          if (!rejectChunk(view, last.fromB)) break;
        }
      },
      acceptAll: () => {
        const current = view.state.doc.toString();
        if (current !== proposed) view.dispatch({ changes: { from: 0, to: current.length, insert: proposed } });
      },
      chunkCount: () => getChunks(view.state)?.chunks.length ?? 0,
    });
    return () => {
      onHandle(null);
      view.destroy();
    };
  }, [path, original, proposed, readOnly, onHandle, onChunksChange]);

  return <div className="editor-host" ref={hostRef} data-testid="merge-editor" />;
}
