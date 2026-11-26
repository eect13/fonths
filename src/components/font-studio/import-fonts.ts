import { toast } from "sonner";
import { folderNameFromFiles, isFontFile } from "@/lib/fonts/fs-drop";
import {
  findImportGoogleCollisions,
  requestImportCollisionChoice,
} from "@/lib/fonts/import-collision";
import {
  analyzeImportBatch,
  filterImportFiles,
  requestImportOptions,
  shouldPromptImportOptions,
} from "@/lib/fonts/import-options";
import { useFontStore } from "@/lib/fonts/store";
import { deactivateSet } from "./activate-toggle";

let inFlight = false;

export async function runFontImport(
  list: FileList | File[],
  opts?: { collectionId?: string; collectionName?: string },
) {
  if (inFlight || useFontStore.getState().uploadBusy) return;
  let files = Array.from(list).filter(isFontFile);
  if (!files.length) {
    toast.error("No TTF, OTF, WOFF, WOFF2, or TTC files found");
    return;
  }

  const st = useFontStore.getState();
  let autoActivate: boolean | undefined = undefined;
  let replaceLocalIds: string[] = [];

  const analysis = analyzeImportBatch(files, st.localFonts);
  if (shouldPromptImportOptions(analysis)) {
    const selection = await requestImportOptions(analysis, {
      mode: "upload",
      folderLabel: opts?.collectionName ?? folderNameFromFiles(files),
    });
    if (!selection) return;
    const filtered = filterImportFiles(files, analysis, selection);
    files = filtered.files;
    autoActivate = selection.autoActivate;
    replaceLocalIds = filtered.replaceExistingLocalIds;
    if (!files.length) {
      toast.message("No font files matched your import options");
      return;
    }
  }

  const { collisions, collidingFileIndices } = findImportGoogleCollisions(
    files,
    st.googleFonts,
    st.activatedSet,
    st.diskFamilySet,
    st.localFonts,
    st.systemFonts,
  );
  if (collisions.length > 0) {
    const choice = await requestImportCollisionChoice(collisions, files.length);
    if (choice === "cancel") return;
    if (choice === "skip-colliding") {
      files = files.filter((_, i) => !collidingFileIndices.has(i));
      if (!files.length) {
        toast.message("Skipped all colliding font files");
        return;
      }
    } else if (choice === "prefer-local") {
      const toDeactivate = collisions
        .filter((c) => c.isGoogleActive)
        .map((c) => c.googleFont.id);
      if (toDeactivate.length) {
        deactivateSet(toDeactivate, "Google Fonts (replaced by local)");
      }
    }
  }

  const collectionName =
    opts?.collectionName ??
    (opts?.collectionId ? undefined : folderNameFromFiles(files));

  inFlight = true;
  try {
    if (replaceLocalIds.length) {
      await useFontStore
        .getState()
        .removeLocalFontsBatch(replaceLocalIds, { deleteFromDisk: true })
        .catch(() => undefined);
    }

    const result = await useFontStore.getState().importFiles(files, {
      collectionId: opts?.collectionId,
      collectionName,
      autoActivate,
    });

    const folder =
      result.collectionId
        ? useFontStore.getState().collections.find((c) => c.id === result.collectionId)
        : undefined;

    if (result.added) {
      toast.success(
        folder
          ? `Added ${result.added} to ${folder.name}`
          : `Added ${result.added} typeface${result.added === 1 ? "" : "s"}`,
      );
    }
    if (result.duplicates) {
      toast.message(
        `${result.duplicates} duplicate file${result.duplicates === 1 ? "" : "s"} skipped`,
      );
    }
    if (result.failed) {
      toast.error(`${result.failed} file${result.failed === 1 ? "" : "s"} could not be read`);
    }
    return result;
  } finally {
    inFlight = false;
  }
}
