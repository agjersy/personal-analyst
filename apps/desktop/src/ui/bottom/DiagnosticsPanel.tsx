import { useState } from "react";
import { backend } from "../../ipc";
import { useAllDiagnostics } from "../../state/diagnostics";
import { useActiveSession, useKernel } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { openFile } from "../workbench/controller";

export function DiagnosticsPanel() {
  const session = useActiveSession();
  const kernel = useKernel(session);
  const diagnostics = useAllDiagnostics(session);
  const [checking, setChecking] = useState(false);
  if (!session) return <div className="empty">No session</div>;
  const check = async () => {
    setChecking(true);
    try {
      await backend().invoke("check_run", { session });
    } catch (e) {
      notifyError(e);
    } finally {
      setChecking(false);
    }
  };
  const errors = diagnostics.filter((d) => d.severity === "error").length;
  return (
    <div className="panel" data-testid="diagnostics-panel">
      <div className="panel-toolbar">
        <span className="small muted">
          {diagnostics.length} problem{diagnostics.length === 1 ? "" : "s"} · {errors} error{errors === 1 ? "" : "s"}
        </span>
        <span className="grow" />
        <button type="button" className="small" onClick={() => void check()} disabled={checking || kernel.state !== "ready"}>
          {checking ? "Checking…" : "Run check"}
        </button>
      </div>
      <div className="panel-body">
        {diagnostics.length === 0 ? <div className="empty">No diagnostics. Run the contract checker or save a Python file.</div> : null}
        {diagnostics.length > 0 ? (
          <table className="grid">
            <thead>
              <tr>
                <th>severity</th>
                <th>file</th>
                <th>line</th>
                <th>rule</th>
                <th>message</th>
              </tr>
            </thead>
            <tbody>
              {diagnostics.map((d, i) => (
                <tr key={`${d.path}:${d.line}:${d.rule}:${i}`} className="clickable" onClick={() => void openFile(d.path, d.line)}>
                  <td>
                    <span className={`badge ${d.severity === "error" ? "error" : "warning"}`}>{d.severity}</span>
                  </td>
                  <td className="mono">{d.path}</td>
                  <td className="mono">
                    {d.line}:{d.col}
                  </td>
                  <td className="mono">{d.rule}</td>
                  <td>
                    {d.message}
                    {d.hint ? <div className="muted">{d.hint}</div> : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </div>
    </div>
  );
}
