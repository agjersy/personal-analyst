import { Channel, invoke } from "@tauri-apps/api/core";
import type { Backend } from "./backend";
import type { Commands } from "./commands";
import type { SessionEvent, SessionId } from "./types";

/**
 * Bridge to the Rust side. Argument objects are passed through untouched, so
 * the snake_case keys documented in IPC.md section 4 reach the commands as is.
 */
export class TauriBackend implements Backend {
  async invoke<K extends keyof Commands>(cmd: K, args: Commands[K]["args"]): Promise<Commands[K]["result"]> {
    return (await invoke(cmd, args as Record<string, any>)) as Commands[K]["result"];
  }

  async subscribe(session: SessionId, onEvent: (e: SessionEvent) => void): Promise<() => void> {
    const channel = new Channel<SessionEvent>();
    channel.onmessage = onEvent;
    const subscription = await invoke<string>("session_subscribe", { session, channel });
    return () => {
      void invoke("session_unsubscribe", { subscription });
    };
  }
}
