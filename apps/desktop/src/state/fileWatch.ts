// Reacts to `files_changed` for open buffers: a path whose on-disk hash still
// matches the buffer's saved hash is our own write and is ignored; otherwise
// the buffer is flagged so the editor shows its reload banner.
import { backend, isIpcError } from "../ipc";
import type { SessionId } from "../ipc";
import { useFiles } from "./files";

export async function checkExternalChanges(session: SessionId, paths: string[]): Promise<void> {
  const files = useFiles.getState();
  const buffers = files.buffers[session];
  if (!buffers) return;
  const open = paths.filter((p) => buffers[p] !== undefined);
  await Promise.all(
    open.map(async (path) => {
      try {
        const fc = await backend().invoke("fs_read", { session, path });
        const current = useFiles.getState().buffers[session]?.[path];
        if (!current || current.hash === fc.hash) return;
        useFiles.getState().markExternal(session, [path], "changed");
      } catch (e) {
        if (isIpcError(e) && e.code === "not_found") useFiles.getState().markExternal(session, [path], "deleted");
      }
    }),
  );
}
