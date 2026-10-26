import { useEffect, useState } from "react";
import { useUi } from "../state/ui";

export type ResolvedTheme = "light" | "dark";

function systemTheme(): ResolvedTheme {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** Applies the preference to <html data-theme> and returns the effective theme. */
export function useResolvedTheme(): ResolvedTheme {
  const pref = useUi((s) => s.theme);
  const [system, setSystem] = useState<ResolvedTheme>(systemTheme);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const handler = () => setSystem(mq.matches ? "dark" : "light");
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);
  const resolved: ResolvedTheme = pref === "system" ? system : pref;
  useEffect(() => {
    const root = document.documentElement;
    if (pref === "system") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", pref);
  }, [pref]);
  return resolved;
}
