import { Suspense, lazy, useEffect, useState } from "react";
import { backend } from "../../ipc";
import type { EqCheck, Equation } from "../../ipc";
import { useFiles } from "../../state/files";
import { useActiveSession, useKernel } from "../../state/sessions";
import { notifyError } from "../../state/ui";
import { openFile } from "../workbench/controller";

const Katex = lazy(() => import("./Katex"));

const TIER_BADGE: Record<Equation["tier"], string> = { derived: "success", declared: "accent", latex: "", none: "" };

export function EquationsPanel() {
  const session = useActiveSession();
  const kernel = useKernel(session);
  const activeFile = useFiles((s) => (session ? (s.activeFile[session] ?? null) : null));
  const revision = useFiles((s) => (session && activeFile ? s.buffers[session]?.[activeFile]?.hash : undefined));
  const [loaded, setLoaded] = useState<{ path: string; revision: string | undefined; equations: Equation[] } | null>(null);
  const [checks, setChecks] = useState<Record<string, EqCheck | "running">>({});
  const path = activeFile?.endsWith(".py") ? activeFile : null;
  const equations = loaded && loaded.path === path ? loaded.equations : [];

  useEffect(() => {
    if (!session || !path || kernel.state !== "ready") return;
    let cancelled = false;
    backend()
      .invoke("eq_extract", { session, path })
      .then((eqs) => {
        if (!cancelled) {
          setLoaded({ path, revision, equations: eqs });
          setChecks({});
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [session, path, revision, kernel.state]);

  if (!session) return <div className="empty">No session</div>;
  if (!path) return <div className="empty">Open a Python file to see its equations.</div>;

  const check = async (fn: string) => {
    setChecks((c) => ({ ...c, [fn]: "running" }));
    try {
      const result = await backend().invoke("eq_check", { session, path, function: fn });
      setChecks((c) => ({ ...c, [fn]: result }));
    } catch (e) {
      notifyError(e);
      setChecks((c) => {
        const next = { ...c };
        delete next[fn];
        return next;
      });
    }
  };

  return (
    <div className="panel" data-testid="equations-panel">
      <div className="panel-toolbar">
        <span className="mono small">{path}</span>
        <span className="grow" />
        <span className="small muted">{equations.length} functions</span>
      </div>
      <div className="panel-body">
        {equations.length === 0 ? <div className="empty">{kernel.state === "ready" ? "No functions found." : `Kernel ${kernel.state}.`}</div> : null}
        {equations.map((eq) => {
          const c = checks[eq.function];
          return (
            <div key={eq.qualname} className="equation">
              <div className="row">
                <button type="button" className="ghost small mono" onClick={() => void openFile(path, eq.line)} title={`Go to line ${eq.line}`}>
                  {eq.qualname}:{eq.line}
                </button>
                <span className={`badge ${TIER_BADGE[eq.tier]}`}>{eq.tier}</span>
                {eq.status !== "ok" ? <span className="badge warning">{eq.status}</span> : null}
                <span className="grow" />
                {c && c !== "running" ? (
                  <span className={`badge ${c.status === "agree" ? "success" : c.status === "mismatch" ? "error" : "warning"}`} title={c.reason ?? `${c.trials} trials, max rel err ${c.max_rel_err}`}>
                    {c.status}
                  </span>
                ) : null}
                <button type="button" className="small" onClick={() => void check(eq.function)} disabled={c === "running" || eq.status !== "ok"}>
                  {c === "running" ? "Checking…" : "Check"}
                </button>
              </div>
              {eq.latex ? (
                <Suspense fallback={<code>{eq.latex}</code>}>
                  <Katex latex={eq.latex} />
                </Suspense>
              ) : (
                <div className="small muted">{eq.reason ?? "no equation"}</div>
              )}
              {c && c !== "running" && c.counterexample ? (
                <pre className="small">counterexample: {JSON.stringify(c.counterexample)}</pre>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}
