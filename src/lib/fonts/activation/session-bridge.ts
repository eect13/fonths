import { inDesktopShell } from "@/lib/desktop/open-fonts";
import type { GdiSessionState } from "./types";

let onBootRunningHook: (() => void) | null = null;

export function setSessionBootPollStarter(fn: () => void) {
  onBootRunningHook = fn;
}

export async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function rememberSessionFamilies(families: string[]): Promise<void> {
  if (!(await inDesktopShell())) return;
  try {
    await tauriInvoke("set_session_families", { families });
  } catch {
    /* older installer */
  }
}

/** Query authoritative Rust in-process GDI session state (live families, faces, GDI handles). */
export async function getGdiSessionState(): Promise<GdiSessionState | null> {
  if (!(await inDesktopShell())) return null;
  try {
    return await tauriInvoke<GdiSessionState>("gdi_session_state");
  } catch {
    return null;
  }
}

export async function listSessionFamilies(): Promise<string[]> {
  if (!(await inDesktopShell())) return [];
  try {
    return (await tauriInvoke<string[]>("session_families")) ?? [];
  } catch {
    return [];
  }
}

/** Poll session_begin GDI restore. Live list is this-process Adds, not last-session sidecar.
 * 1.0.190: onReady fires as session_boot.ready grows — do not wait for boot.done (~2099). */
export async function waitSessionBoot(
  timeoutMs = 180_000,
  onReady?: (ready: string[], done: boolean) => void,
): Promise<{ done: boolean; ready: string[] }> {
  if (!(await inDesktopShell())) {
    onReady?.([], true);
    return { done: true, ready: [] };
  }
  const start = Date.now();
  let startedPoll = false;
  let lastReadyLen = -1;
  let latest: string[] = [];
  while (Date.now() - start < timeoutMs) {
    try {
      const s = await tauriInvoke<{ done?: boolean; running?: boolean; ready?: string[] }>(
        "session_boot_state",
      );
      if (s?.running && !startedPoll) {
        onBootRunningHook?.();
        startedPoll = true;
      }
      latest = (s?.ready ?? []).slice();
      const done = Boolean(s?.done);
      if (latest.length !== lastReadyLen || done) {
        lastReadyLen = latest.length;
        onReady?.(latest, done);
      }
      if (done) return { done: true, ready: latest };
    } catch {
      onReady?.([], false);
      return { done: false, ready: [] };
    }
    // Slightly slower than 150ms — pairs with Rust emit throttle; keeps webview interactive.
    await new Promise((r) => window.setTimeout(r, 250));
  }
  onReady?.(latest, false);
  return { done: false, ready: latest };
}

/** Soft Settled Power Retry: clear Rust this-session Add=0 refuse so Add runs again (1.0.206f). */
export async function clearSessionGdiRefused(family: string): Promise<void> {
  const name = family.trim();
  if (!name) return;
  try {
    await tauriInvoke("clear_session_gdi_refused_family", { family: name });
  } catch {
    /* web preview / no Tauri */
  }
}
