import { useEffect, useState } from "react";
import { errorMessage } from "./ipc";
import { bootstrap } from "./state/actions";
import { useActiveSession, useSessions } from "./state/sessions";
import { useUi } from "./state/ui";
import { Toasts } from "./ui/Toasts";
import { Palette } from "./ui/palette/Palette";
import { useResolvedTheme } from "./ui/theme";
import { NewSessionDialog } from "./ui/topbar/NewSessionDialog";
import { SettingsDialog } from "./ui/topbar/SettingsDialog";
import { TopBar } from "./ui/topbar/TopBar";
import { useAppCommands } from "./ui/useAppCommands";
import { Workbench } from "./ui/workbench/Workbench";

export function App() {
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const active = useActiveSession();
  const sessions = useSessions((s) => s.sessions);
  const newSessionOpen = useUi((s) => s.newSessionOpen);
  const settingsOpen = useUi((s) => s.settingsOpen);
  useResolvedTheme();
  useAppCommands();

  useEffect(() => {
    bootstrap()
      .then(() => setReady(true))
      .catch((e: unknown) => setError(errorMessage(e)));
  }, []);

  return (
    <div className="app">
      <TopBar />
      <div className="app-body">
        {error ? <div className="welcome">{error}</div> : null}
        {!error && !ready ? <div className="welcome muted">Starting…</div> : null}
        {ready && active ? <Workbench key={active} session={active} /> : null}
        {ready && !active ? (
          <div className="welcome" data-testid="welcome">
            <h1>Personal Analyst</h1>
            <p>{sessions.length ? "Pick a session above, or create a new one." : "Create a session to start."}</p>
            <button type="button" className="primary" onClick={() => useUi.getState().setNewSessionOpen(true)}>
              New session
            </button>
          </div>
        ) : null}
      </div>
      {newSessionOpen ? <NewSessionDialog onClose={() => useUi.getState().setNewSessionOpen(false)} /> : null}
      {settingsOpen ? <SettingsDialog session={active} onClose={() => useUi.getState().setSettingsOpen(false)} /> : null}
      <Palette />
      <Toasts />
    </div>
  );
}
