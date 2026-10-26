import { DockviewDefaultTab, DockviewReact, themeDark, themeLight } from "dockview-react";
import type { DockviewApi, DockviewReadyEvent, IDockviewPanelHeaderProps, IDockviewPanelProps } from "dockview-react";
import { useCallback, useEffect, useRef } from "react";
import { backend } from "../../ipc";
import type { SessionId } from "../../ipc";
import { useFiles } from "../../state/files";
import { useResolvedTheme } from "../theme";
import { ChatPanel } from "../chat/ChatPanel";
import { DiagnosticsPanel } from "../bottom/DiagnosticsPanel";
import { RunOutputPanel } from "../bottom/RunOutputPanel";
import { TerminalPanel } from "../bottom/TerminalPanel";
import { EditorPanel } from "../editor/EditorPanel";
import { ProposalReviewPanel } from "../editor/ProposalReviewPanel";
import { EquationsPanel } from "../panels/EquationsPanel";
import { FiguresPanel } from "../panels/FiguresPanel";
import { PackagesPanel } from "../panels/PackagesPanel";
import { LibraryView } from "../sidebar/LibraryView";
import { ProjectView } from "../sidebar/ProjectView";
import { RunsView } from "../sidebar/RunsView";
import { ScriptsView } from "../sidebar/ScriptsView";
import { GROUPS, setWorkbench } from "./controller";

const components: Record<string, React.FunctionComponent<IDockviewPanelProps>> = {
  project: ProjectView,
  scripts: ScriptsView,
  library: LibraryView,
  runs: RunsView,
  editor: EditorPanel,
  proposal: ProposalReviewPanel,
  chat: ChatPanel,
  terminal: TerminalPanel,
  runoutput: RunOutputPanel,
  diagnostics: DiagnosticsPanel,
  figures: FiguresPanel,
  equations: EquationsPanel,
  packages: PackagesPanel,
};

function FixedTab(props: IDockviewPanelHeaderProps) {
  return <DockviewDefaultTab {...props} hideClose />;
}

const tabComponents: Record<string, React.FunctionComponent<IDockviewPanelHeaderProps>> = { fixed: FixedTab };

function Watermark() {
  return (
    <div className="welcome">
      <div className="muted">Open a file from the Project view, or press ⌘/Ctrl+P</div>
    </div>
  );
}

export function buildDefaultLayout(api: DockviewApi): void {
  api.clear();
  const editors = api.addGroup({ id: GROUPS.editors, direction: "right" });
  const sidebar = api.addGroup({ id: GROUPS.sidebar, referenceGroup: GROUPS.editors, direction: "left" });
  api.addPanel({ id: "project", component: "project", tabComponent: "fixed", title: "Project", position: { referenceGroup: GROUPS.sidebar } });
  api.addPanel({ id: "scripts", component: "scripts", tabComponent: "fixed", title: "Scripts", position: { referenceGroup: GROUPS.sidebar }, inactive: true });
  api.addPanel({ id: "library", component: "library", tabComponent: "fixed", title: "Library", position: { referenceGroup: GROUPS.sidebar }, inactive: true });
  api.addPanel({ id: "runs", component: "runs", tabComponent: "fixed", title: "Runs", position: { referenceGroup: GROUPS.sidebar }, inactive: true });
  const chat = api.addGroup({ id: GROUPS.chat, referenceGroup: GROUPS.editors, direction: "right" });
  api.addPanel({ id: "chat", component: "chat", tabComponent: "fixed", title: "Agent", position: { referenceGroup: GROUPS.chat } });
  const bottom = api.addGroup({ id: GROUPS.bottom, referenceGroup: GROUPS.editors, direction: "below" });
  api.addPanel({ id: "terminal", component: "terminal", tabComponent: "fixed", title: "Terminal", position: { referenceGroup: GROUPS.bottom } });
  api.addPanel({ id: "runoutput", component: "runoutput", tabComponent: "fixed", title: "Run output", position: { referenceGroup: GROUPS.bottom }, inactive: true });
  api.addPanel({ id: "diagnostics", component: "diagnostics", tabComponent: "fixed", title: "Diagnostics", position: { referenceGroup: GROUPS.bottom }, inactive: true });
  const panels = api.addGroup({ id: GROUPS.panels, direction: "below" });
  api.addPanel({ id: "figures", component: "figures", tabComponent: "fixed", title: "Figures", position: { referenceGroup: GROUPS.panels } });
  api.addPanel({ id: "equations", component: "equations", tabComponent: "fixed", title: "Equations", position: { referenceGroup: GROUPS.panels }, inactive: true });
  api.addPanel({ id: "packages", component: "packages", tabComponent: "fixed", title: "Packages", position: { referenceGroup: GROUPS.panels }, inactive: true });
  sidebar.api.setSize({ width: 300 });
  chat.api.setSize({ width: 400 });
  bottom.api.setSize({ height: 220 });
  panels.api.setSize({ height: 260 });
  editors.api.setActive();
}

export function Workbench({ session }: { session: SessionId }) {
  const theme = useResolvedTheme();
  const apiRef = useRef<DockviewApi | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const restoring = useRef(false);

  const onReady = useCallback(
    (event: DockviewReadyEvent) => {
      const api = event.api;
      apiRef.current = api;
      setWorkbench(api, session);
      void (async () => {
        let restored = false;
        try {
          const saved = await backend().invoke("layout_get", { session });
          if (saved) {
            restoring.current = true;
            api.fromJSON(JSON.parse(saved));
            restored = api.getGroup(GROUPS.editors) !== undefined;
          }
        } catch {
          restored = false;
        } finally {
          restoring.current = false;
        }
        if (!restored) buildDefaultLayout(api);
      })();
      api.onDidLayoutChange(() => {
        if (restoring.current) return;
        if (saveTimer.current) clearTimeout(saveTimer.current);
        saveTimer.current = setTimeout(() => {
          const layout = JSON.stringify(api.toJSON());
          backend()
            .invoke("layout_save", { session, layout })
            .catch(() => undefined);
        }, 600);
      });
      api.onDidActivePanelChange((e) => {
        const panel = e.panel;
        if (panel && panel.api.component === "editor") {
          const path = (panel.params as { path?: string } | undefined)?.path;
          if (path) useFiles.getState().setActiveFile(session, path);
        }
      });
    },
    [session],
  );

  useEffect(
    () => () => {
      setWorkbench(null, null);
      if (saveTimer.current) clearTimeout(saveTimer.current);
    },
    [],
  );

  return (
    <div className="workbench" data-testid="workbench">
      <DockviewReact
        components={components}
        tabComponents={tabComponents}
        watermarkComponent={Watermark}
        onReady={onReady}
        theme={theme === "dark" ? themeDark : themeLight}
        closeEmptyGroups={false}
        noPanelsOverlay="watermark"
        disableFloatingGroups={false}
      />
    </div>
  );
}
