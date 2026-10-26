import { useIndex } from "../../state/files";
import { useActiveSession } from "../../state/sessions";
import { openFile } from "../workbench/controller";

export function LibraryView() {
  const session = useActiveSession();
  const index = useIndex(session);
  if (!session) return <div className="empty">No session</div>;
  return (
    <div className="panel">
      <div className="panel-toolbar">
        <span className="grow small muted">Library{index ? ` (${index.library.length})` : ""}</span>
      </div>
      <div className="panel-body">
        {!index ? <div className="empty">Indexing…</div> : index.library.length === 0 ? <div className="empty">No public functions</div> : null}
        <ul className="list">
          {index?.library.map((fn) => (
            <li key={`${fn.path}:${fn.qualname}`}>
              <button type="button" className="list-item" onClick={() => void openFile(fn.path, fn.line)} title={`${fn.path}:${fn.line}`}>
                <div className="grow" style={{ padding: "3px 0" }}>
                  <div className="row">
                    <span className="mono truncate">{fn.signature}</span>
                    {fn.has_math ? <span className="badge accent">Math</span> : null}
                    <span className="badge" title="call sites">
                      {fn.callers.length} caller{fn.callers.length === 1 ? "" : "s"}
                    </span>
                  </div>
                  <div className="small muted truncate">
                    {fn.module} · {fn.doc_summary ?? "no docstring"}
                  </div>
                </div>
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
