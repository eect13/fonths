import { toast } from "sonner";
import { inDesktopShell } from "@/lib/desktop/open-fonts";
import { tauriInvoke } from "./session-bridge";
import type { DiskFamilyInfo } from "./types";

export async function managedDocumentsRoot(): Promise<string | null> {
  if (!(await inDesktopShell())) return null;
  try {
    return (await tauriInvoke<string>("activation_folder")) ?? null;
  } catch {
    return null;
  }
}

export async function scanDiskFamilies(): Promise<DiskFamilyInfo[]> {
  if (!(await inDesktopShell())) return [];
  try {
    return (await tauriInvoke<DiskFamilyInfo[]>("scan_disk_families")) ?? [];
  } catch {
    return [];
  }
}

export async function pruneUnknownFolders(keep: string[]): Promise<number> {
  if (!(await inDesktopShell())) return 0;
  try {
    return (await tauriInvoke<number>("prune_unknown_folders", { keep })) ?? 0;
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err ?? "prune failed");
    toast.error("Could not prune Documents folders", { description: msg });
    return 0;
  }
}

export async function openActivatedFolder(sub?: "google" | "uploads"): Promise<string | null> {
  if (!(await inDesktopShell())) return null;
  try {
    const dir = (await tauriInvoke<string>("activation_folder")) ?? null;
    await tauriInvoke("open_activation_folder", { sub: sub ?? null });
    return dir;
  } catch {
    return null;
  }
}
