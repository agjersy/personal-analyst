import { Suspense, lazy } from "react";
import { closeTerminal, openTerminal } from "../../state/actions";
import { useActiveSession, useKernel } from "../../state/sessions";
import { useTerminals } from "../../state/terminals";
import type { TerminalTab } from "../../state/terminals";

const XtermView = lazy(() => import("./XtermView"));

const NO_TERMINALS: TerminalTab[] = [];

export function TerminalPanel() {
  const session = useActiveSession();
  const kernel = useKernel(session);
  const terminals = useTerminals((s) => (session ? (s.terminals[session] ?? NO_TERMINALS) : NO_TERMINALS));
  const active = useTerminals((s) => (session ? (s.active[session] ?? null) : null));
  if (!session) return <div className="empty">No session</div>;
  const current = terminals.find((t) => t.id === active) ?? terminals[terminals.length - 1];
  return (
    <div className="panel" data-testid="terminal-panel">
      <div className="terminal-tabs">
        {terminals.map((t) => (
          <button
            key={t.id}
            type="button"
            className={`terminal-tab small${current?.id === t.id ? " active" : ""}`}
            onClick={() => useTerminals.getState().setActive(session, t.id)}
          >
            {t.title}
            {t.exit !== undefined ? " (exited)" : ""}
          </button>
        ))}
        <button type="button" className="ghost small" onClick={() => void openTerminal(session)} aria-label="New terminal" title="New terminal (Ctrl+`)">
          +
        </button>
        <span className="grow" />
        {current ? (
          <button type="button" className="ghost small" onClick={() => void closeTerminal(session, current.id)} aria-label="Close terminal">
            ×
          </button>
        ) : null}
      </div>
      {current ? (
        <Suspense fallback={<div className="empty">Loading terminal…</div>}>
          <XtermView key={current.id} terminalId={current.id} exited={current.exit} />
        </Suspense>
      ) : (
        <div className="empty">
          <p>A shell on the session target with the venv active.</p>
          <button type="button" onClick={() => void openTerminal(session)} disabled={kernel.state === "stopped"}>
            Open terminal
          </button>
        </div>
      )}
    </div>
  );
}
