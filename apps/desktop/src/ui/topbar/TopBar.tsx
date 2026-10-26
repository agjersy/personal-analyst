import { backend } from "../../ipc";
import { closeSession, openSession } from "../../state/actions";
import { useSessions } from "../../state/sessions";
import { notifyError, useUi } from "../../state/ui";

export function TopBar() {
  const sessions = useSessions((s) => s.sessions);
  const infos = useSessions((s) => s.infos);
  const kernel = useSessions((s) => s.kernel);
  const active = useSessions((s) => s.active);
  const models = useSessions((s) => s.models);
  const info = active ? infos[active] : undefined;

  const setModel = (model: string) => {
    if (!active || !info) return;
    backend()
      .invoke("chat_set_model", { session: active, conversation: info.active_conversation, model })
      .catch(notifyError);
  };

  return (
    <header className="topbar" data-testid="topbar">
      <div className="session-tabs" role="tablist" aria-label="Sessions">
        {sessions.map((s) => {
          const k = kernel[s.id]?.state ?? "stopped";
          const isActive = s.id === active;
          return (
            <button
              key={s.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              className={`session-tab${isActive ? " active" : ""}`}
              onClick={() => (s.open && infos[s.id] ? useSessions.getState().setActive(s.id) : void openSession(s.id).catch(notifyError))}
              title={`${s.repo_abs} · ${s.target} · kernel ${k}`}
              data-testid="session-tab"
            >
              <span className={`dot ${s.open ? k : ""}`} aria-label={`kernel ${k}`} />
              <span>{s.name}</span>
              <span className="target">{s.target}</span>
              {s.open ? (
                <span
                  className="close"
                  role="button"
                  tabIndex={0}
                  aria-label={`Close session ${s.name}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    void closeSession(s.id).catch(notifyError);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.stopPropagation();
                      void closeSession(s.id).catch(notifyError);
                    }
                  }}
                >
                  ×
                </span>
              ) : null}
            </button>
          );
        })}
        <button type="button" className="ghost icon" onClick={() => useUi.getState().setNewSessionOpen(true)} aria-label="New session" title="New session" data-testid="new-session">
          +
        </button>
      </div>
      <div className="topbar-right">
        {info ? (
          <select aria-label="Model" value={info.model ?? ""} onChange={(e) => setModel(e.target.value)} data-testid="model-picker">
            {models.map((m) => (
              <option key={`${m.provider}/${m.id}`} value={`${m.provider}/${m.id}`}>
                {m.name}
                {m.local ? " (local)" : ""}
              </option>
            ))}
          </select>
        ) : null}
        <button type="button" className="ghost icon" onClick={() => useUi.getState().setSettingsOpen(true)} aria-label="Settings" title="Settings">
          ⚙
        </button>
      </div>
    </header>
  );
}
