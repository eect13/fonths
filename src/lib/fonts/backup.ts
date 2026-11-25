import { toast } from "sonner";
import { inDesktopShell } from "@/lib/desktop/open-fonts";
import { applyDesktopPrefs } from "@/lib/desktop/prefs";
import {
  buildLibraryBackup,
  FONTHS_BACKUP_VERSION,
  mergeLibraryBackupState,
  parseLibraryBackup,
  type BackupCollection,
  type BackupFontSet,
  type LibraryBackup,
} from "./backup-schema";
import { useFontStore } from "./store";

export {
  buildLibraryBackup,
  FONTHS_BACKUP_VERSION,
  mergeLibraryBackupState,
  parseLibraryBackup,
  type BackupCollection,
  type BackupFontSet,
  type LibraryBackup,
};

export function applyLibraryBackup(backup: LibraryBackup): {
  favorites: number;
  collections: number;
  tags: number;
  features: number;
} {
  const current = useFontStore.getState();
  const merged = mergeLibraryBackupState(current, backup);
  useFontStore.setState({
    favorites: merged.favorites,
    collections: merged.collections,
    fontSets: merged.fontSets,
    customTags: merged.customTags,
    featurePrefs: merged.featurePrefs,
    previewAxes: merged.previewAxes,
    desktopPrefs: merged.desktopPrefs,
  });
  useFontStore.getState().setAutoHideDuplicates(merged.autoHideDuplicates);
  void applyDesktopPrefs(merged.desktopPrefs);
  return {
    favorites: backup.favorites.length,
    collections: backup.collections.length,
    tags: Object.keys(backup.customTags).length,
    features: Object.keys(backup.featurePrefs).length,
  };
}

export async function exportLibraryBackup(): Promise<boolean> {
  const backup = buildLibraryBackup(useFontStore.getState());
  const text = `${JSON.stringify(backup, null, 2)}\n`;
  const stamp = new Date().toISOString().slice(0, 10);
  const fileName = `fonths-backup-${stamp}.json`;

  if (await inDesktopShell()) {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      const dest = await save({
        defaultPath: fileName,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!dest) return false;
      await writeTextFile(dest, text);
      toast.success("Library backup exported", {
        description: `${backup.favorites.length} favorites · ${backup.collections.length} collections · ${Object.keys(backup.customTags).length} tagged families`,
      });
      return true;
    } catch {
      /* fall through to browser download */
    }
  }

  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName;
  a.click();
  URL.revokeObjectURL(url);
  toast.success("Library backup exported", {
    description: `${backup.favorites.length} favorites · ${backup.collections.length} collections`,
  });
  return true;
}

export async function importLibraryBackup(): Promise<boolean> {
  let text = "";
  if (await inDesktopShell()) {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { readTextFile } = await import("@tauri-apps/plugin-fs");
      const path = await open({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path || Array.isArray(path)) return false;
      text = await readTextFile(path);
    } catch {
      text = "";
    }
  }

  if (!text) {
    text = await new Promise<string>((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.accept = "application/json,.json";
      input.onchange = () => {
        const file = input.files?.[0];
        if (!file) return resolve("");
        void file.text().then(resolve, () => resolve(""));
      };
      input.click();
    });
  }
  if (!text) return false;

  let backup: LibraryBackup;
  try {
    backup = parseLibraryBackup(text);
  } catch (err) {
    toast.error("Invalid Fonths backup file", {
      description: err instanceof Error ? err.message : String(err),
    });
    return false;
  }

  const summary = applyLibraryBackup(backup);
  toast.success("Backup restored", {
    description: `${summary.favorites} favorites · ${summary.collections} collections · ${summary.tags} custom tags · settings updated`,
  });
  return true;
}
