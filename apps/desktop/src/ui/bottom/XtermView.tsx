import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { backend } from "../../ipc";
import type { TerminalId } from "../../ipc";
import { attachTerminalSink } from "../../state/terminals";
import { useResolvedTheme } from "../theme";

function themeFor(dark: boolean) {
  return dark
    ? { background: "#1b1b1c", foreground: "#d8d8d8", cursor: "#d8d8d8", selectionBackground: "#2f4262" }
    : { background: "#ffffff", foreground: "#2b2b2b", cursor: "#2b2b2b", selectionBackground: "#cfe0fa" };
}

export default function XtermView({ terminalId, exited }: { terminalId: TerminalId; exited: number | null | undefined }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const theme = useResolvedTheme();

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const term = new Terminal({
      cursorBlink: true,
      convertEol: false,
      fontFamily: '"SF Mono", "JetBrains Mono", Menlo, Consolas, "Liberation Mono", monospace',
      fontSize: 12.5,
      scrollback: 5000,
      theme: themeFor(theme === "dark"),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    termRef.current = term;
    const detach = attachTerminalSink(terminalId, (data) => term.write(data));
    // Writes are serialised and coalesced so keystrokes cannot overtake each
    // other on the way to the pty.
    let pending = "";
    let inflight = false;
    const flush = () => {
      if (inflight || pending === "") return;
      const data = pending;
      pending = "";
      inflight = true;
      backend()
        .invoke("terminal_write", { terminal_id: terminalId, data })
        .catch(() => undefined)
        .finally(() => {
          inflight = false;
          flush();
        });
    };
    const onData = term.onData((data) => {
      pending += data;
      flush();
    });
    const onResize = term.onResize(({ cols, rows }) => {
      backend()
        .invoke("terminal_resize", { terminal_id: terminalId, cols, rows })
        .catch(() => undefined);
    });
    const doFit = () => {
      try {
        fit.fit();
      } catch {
        // host not laid out yet
      }
    };
    const observer = new ResizeObserver(doFit);
    observer.observe(host);
    doFit();
    term.focus();
    return () => {
      observer.disconnect();
      onData.dispose();
      onResize.dispose();
      detach();
      term.dispose();
      termRef.current = null;
    };
    // The terminal is created once; theme changes are applied below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [terminalId]);

  useEffect(() => {
    const term = termRef.current;
    if (term) term.options.theme = themeFor(theme === "dark");
  }, [theme]);

  useEffect(() => {
    if (exited !== undefined) termRef.current?.write(`\r\n\x1b[90m[process exited${exited === null ? "" : ` with code ${exited}`}]\x1b[0m\r\n`);
  }, [exited]);

  return <div className="terminal-host" ref={hostRef} data-testid="xterm" />;
}
