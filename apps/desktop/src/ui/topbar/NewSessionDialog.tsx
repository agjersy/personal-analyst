import { useEffect, useRef, useState } from "react";
import { backend, errorMessage } from "../../ipc";
import { createSession } from "../../state/actions";
import { useSessions } from "../../state/sessions";
import { useUi } from "../../state/ui";
import { Modal } from "../common/Dialog";

export function NewSessionDialog({ onClose }: { onClose: () => void }) {
  const models = useSessions((s) => s.models);
  const targets = useSessions((s) => s.targets);
  const app = useSessions((s) => s.app);
  const [repo, setRepo] = useState("");
  const [target, setTarget] = useState("local");
  const [model, setModel] = useState(models[0] ? `${models[0].provider}/${models[0].id}` : "");
  const [init, setInit] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  const browse = async () => {
    try {
      const picked = await backend().invoke("dialog_pick_directory", { title: "Choose an analysis repository" });
      if (picked) setRepo(picked);
      else if (!app || app.dev_mode) useUi.getState().toast("info", "No native folder picker here; type an absolute path.");
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const create = async () => {
    if (!repo.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await createSession({ repo_abs: repo.trim(), target, ...(model ? { model } : {}), init });
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title="New session"
      onClose={onClose}
      actions={
        <>
          <button type="button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="button" className="primary" onClick={() => void create()} disabled={busy || !repo.trim()} data-testid="create-session">
            {busy ? "Creating…" : "Create session"}
          </button>
        </>
      }
    >
      <div className="form-grid">
        <label htmlFor="ns-repo">Repository</label>
        <div className="row">
          <input
            id="ns-repo"
            ref={inputRef}
            type="text"
            className="grow"
            placeholder={app ? `${app.home_abs}/projects/my-analysis` : "/absolute/path"}
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void create()}
          />
          <button type="button" onClick={() => void browse()}>
            Browse…
          </button>
        </div>
        <div className="help">Absolute path of the analysis repo on this machine. Data stays on the target.</div>
        <label htmlFor="ns-target">Target</label>
        <select id="ns-target" value={target} onChange={(e) => setTarget(e.target.value)}>
          {targets.map((t) => (
            <option key={t.name} value={t.name}>
              {t.name} {t.kind === "ssh" ? `(ssh ${t.host ?? ""})` : "(this machine)"}
              {t.bootstrapped === false ? " · not bootstrapped" : ""}
            </option>
          ))}
        </select>
        <label htmlFor="ns-model">Model</label>
        <select id="ns-model" value={model} onChange={(e) => setModel(e.target.value)}>
          {models.map((m) => (
            <option key={`${m.provider}/${m.id}`} value={`${m.provider}/${m.id}`}>
              {m.name} · {m.provider}
              {m.local ? " · local" : ""}
            </option>
          ))}
        </select>
        <span />
        <label className="row">
          <input type="checkbox" checked={init} onChange={(e) => setInit(e.target.checked)} />
          Initialize as an analysis repo (runs the <code>analyst init</code> template on an empty directory)
        </label>
      </div>
      {error ? (
        <div className="small" style={{ color: "var(--error)" }} role="alert">
          {error}
        </div>
      ) : null}
    </Modal>
  );
}
