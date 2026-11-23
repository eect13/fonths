import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  analyzeImportBatch,
  classifyFontFile,
  defaultImportSelection,
  filterImportFiles,
  filterOriginPathsByRule,
  selectionFromSavedRule,
  shouldPromptImportOptions,
} from "../src/lib/fonts/import-options.ts";
import {
  createToastGuardState,
  evaluateToastGuard,
  MAX_TOAST_DESC_LEN,
  MAX_TOAST_TITLE_LEN,
  sanitizeToastText,
  TOAST_BURST_ID,
  TOAST_BURST_LIMIT,
} from "../src/lib/toast-guard.ts";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

test("Smart Import: classifies variable, static, italic, and web formats accurately", () => {
  const vf = classifyFontFile({ name: "Inter-VariableFont_opsz,wght.ttf", size: 320_000 }, 0);
  assert.equal(vf.familyKey, "inter");
  assert.equal(vf.isVariable, true);
  assert.equal(vf.isItalic, false);
  assert.equal(vf.ext, "ttf");
  assert.equal(vf.isWebOnly, false);

  const staticSub = classifyFontFile(
    { name: "Inter_18pt-BoldItalic.ttf", webkitRelativePath: "Inter/static/Inter_18pt-BoldItalic.ttf", size: 90_000 },
    1,
  );
  assert.equal(staticSub.familyKey, "inter");
  assert.equal(staticSub.isVariable, false);
  assert.equal(staticSub.isItalic, true);

  const web = classifyFontFile({ name: "PlusJakartaSans-Regular.woff2", size: 42_000 }, 2);
  assert.equal(web.familyKey, "plusjakartasans");
  assert.equal(web.ext, "woff2");
  assert.equal(web.isWebOnly, true);
  assert.equal(web.isVariable, false);

  const bracket = classifyFontFile({ name: "Roboto-Italic[wdth,wght].ttf", size: 210_000 }, 3);
  assert.equal(bracket.familyKey, "roboto");
  assert.equal(bracket.isVariable, true);
  assert.equal(bracket.isItalic, true);
});

test("Smart Import: zero-friction bypass for small clean single-format batch with no Var/Static overlap", () => {
  const files = [
    { name: "Figtree-wght.ttf", size: 180_000 },
    { name: "Inter-opsz-wght.ttf", size: 300_000 },
  ];
  const analysis = analyzeImportBatch(files, []);
  assert.equal(analysis.varStaticConflicts.length, 0);
  assert.equal(shouldPromptImportOptions(analysis), false);
});

test("Smart Import: within-batch Variable + Static conflict prompts and defaults to Variable only", () => {
  const files = [
    { name: "Inter-VariableFont_opsz,wght.ttf", size: 340_000 },
    { name: "Inter-Italic-VariableFont_opsz,wght.ttf", size: 350_000 },
    { name: "Inter_18pt-Regular.ttf", webkitRelativePath: "Inter/static/Inter_18pt-Regular.ttf", size: 80_000 },
    { name: "Inter_18pt-Bold.ttf", webkitRelativePath: "Inter/static/Inter_18pt-Bold.ttf", size: 82_000 },
  ];
  const analysis = analyzeImportBatch(files, []);
  assert.equal(analysis.varStaticConflicts.length, 1);
  assert.equal(analysis.varStaticConflicts[0].familyKey, "inter");
  assert.equal(analysis.varStaticConflicts[0].batchVariableCount, 2);
  assert.equal(analysis.varStaticConflicts[0].batchStaticCount, 2);
  assert.equal(shouldPromptImportOptions(analysis), true);

  const defaults = defaultImportSelection(analysis);
  assert.equal(defaults.varStaticMode, "variable");

  const varOnly = filterImportFiles(files, analysis, defaults);
  assert.deepEqual(
    varOnly.files.map((f) => f.name),
    ["Inter-VariableFont_opsz,wght.ttf", "Inter-Italic-VariableFont_opsz,wght.ttf"],
  );

  const staticOnly = filterImportFiles(files, analysis, { ...defaults, varStaticMode: "static" });
  assert.deepEqual(
    staticOnly.files.map((f) => f.name),
    ["Inter_18pt-Regular.ttf", "Inter_18pt-Bold.ttf"],
  );

  const noItalic = filterImportFiles(files, analysis, {
    ...defaults,
    varStaticMode: "both",
    includeItalic: false,
  });
  assert.deepEqual(
    noItalic.files.map((f) => f.name),
    ["Inter-VariableFont_opsz,wght.ttf", "Inter_18pt-Regular.ttf", "Inter_18pt-Bold.ttf"],
  );
});

test("Smart Import: cross-library conflict detects existing static vs incoming variable and vice versa", () => {
  const existingStatic = [
    { id: "l-static-1", family: "Inter", variable: false },
    { id: "l-static-2", family: "Inter", variable: false },
  ];
  const incomingVar = [{ name: "Inter-VariableFont_wght.ttf", size: 280_000 }];
  const analysis1 = analyzeImportBatch(incomingVar, existingStatic);
  assert.equal(analysis1.varStaticConflicts.length, 1);
  assert.equal(shouldPromptImportOptions(analysis1), true);
  const sel1 = defaultImportSelection(analysis1);
  assert.equal(sel1.varStaticMode, "variable");
  assert.equal(sel1.replaceExistingOpposite, true);
  const res1 = filterImportFiles(incomingVar, analysis1, sel1);
  assert.equal(res1.files.length, 1);
  assert.deepEqual(res1.replaceExistingLocalIds, ["l-static-1", "l-static-2"]);

  // Reverse: existing Variable in library, user drops Static files -> defaults to Static so CTA is enabled.
  const existingVar = [{ id: "l-var-1", family: "Inter", variable: true }];
  const incomingStatic = [
    { name: "Inter-Regular.ttf", size: 80_000 },
    { name: "Inter-Bold.ttf", size: 82_000 },
  ];
  const analysis2 = analyzeImportBatch(incomingStatic, existingVar);
  assert.equal(analysis2.varStaticConflicts.length, 1);
  assert.equal(shouldPromptImportOptions(analysis2), true);
  const sel2 = defaultImportSelection(analysis2);
  assert.equal(sel2.varStaticMode, "static");
  const res2 = filterImportFiles(incomingStatic, analysis2, sel2);
  assert.equal(res2.files.length, 2);
  assert.deepEqual(res2.replaceExistingLocalIds, ["l-var-1"]);
});

test("Smart Import: mixed desktop + web formats auto-unchecks WOFF/WOFF2, web-only keeps WOFF2 checked", () => {
  const mixed = [
    { name: "Acme-Regular.otf", size: 65_000 },
    { name: "Acme-Regular.woff2", size: 25_000 },
  ];
  const mixedAnalysis = analyzeImportBatch(mixed, []);
  assert.equal(shouldPromptImportOptions(mixedAnalysis), true);
  const mixedSel = defaultImportSelection(mixedAnalysis);
  assert.equal(mixedSel.formats.otf, true);
  assert.equal(mixedSel.formats.woff2, false);
  const filteredMixed = filterImportFiles(mixed, mixedAnalysis, mixedSel);
  assert.deepEqual(
    filteredMixed.files.map((f) => f.name),
    ["Acme-Regular.otf"],
  );

  const webOnly = [{ name: "Acme-Regular.woff2", size: 25_000 }];
  const webAnalysis = analyzeImportBatch(webOnly, []);
  const webSel = defaultImportSelection(webAnalysis);
  assert.equal(webSel.formats.woff2, true);
});

test("Toast Guard: sanitizes Error objects, control chars, and truncates huge strings", () => {
  assert.equal(sanitizeToastText(new Error("Access\u0000 denied\n\t  here"), 50), "Access denied here");
  const huge = "X".repeat(MAX_TOAST_TITLE_LEN + 200);
  const clipped = sanitizeToastText(huge, MAX_TOAST_TITLE_LEN);
  assert.equal(clipped.length, MAX_TOAST_TITLE_LEN);
  assert.ok(clipped.endsWith("…"));
  const hugeDesc = "Y".repeat(MAX_TOAST_DESC_LEN + 100);
  assert.equal(sanitizeToastText(hugeDesc, MAX_TOAST_DESC_LEN).length, MAX_TOAST_DESC_LEN);
  assert.equal(sanitizeToastText(null, 50), "");
});

test("Toast Guard: deduplicates repeated messages and coalesces bursts to prevent crashes", () => {
  const state = createToastGuardState();
  const t0 = 10_000;

  // Explicit IDs always pass through unchanged.
  const explicit = evaluateToastGuard(state, t0, "progress", "job-1");
  assert.equal(explicit.action, "pass");
  assert.equal(explicit.id, "job-1");

  // Duplicate un-ID'd messages within 1200ms reuse the same dedupe ID.
  const first = evaluateToastGuard(state, t0, "success|Moved A");
  const dup = evaluateToastGuard(state, t0 + 200, "success|Moved A");
  assert.equal(first.action, "pass");
  assert.equal(dup.action, "pass");
  assert.equal(first.id, dup.id);

  // Fill up to TOAST_BURST_LIMIT distinct messages inside 1000ms.
  for (let i = 1; i < TOAST_BURST_LIMIT; i += 1) {
    const d = evaluateToastGuard(state, t0 + i * 20, `success|Moved Font ${i}`);
    assert.equal(d.action, "pass");
  }

  // Next distinct toast within the burst window MUST coalesce into ONE summary toast.
  const burst1 = evaluateToastGuard(state, t0 + 300, "success|Moved Font OVERFLOW 1");
  assert.equal(burst1.action, "coalesce");
  assert.equal(burst1.id, TOAST_BURST_ID);
  assert.equal(burst1.count, TOAST_BURST_LIMIT + 1);

  const burst2 = evaluateToastGuard(state, t0 + 350, "success|Moved Font OVERFLOW 2");
  assert.equal(burst2.action, "coalesce");
  assert.equal(burst2.id, TOAST_BURST_ID);
  assert.equal(burst2.count, TOAST_BURST_LIMIT + 2);

  // After burst window clears, normal toasts pass again.
  const later = evaluateToastGuard(state, t0 + 3_500, "success|Fresh Toast");
  assert.equal(later.action, "pass");
});

test("Delete & Reset Library contracts: batched disk cleanup, queue cancellation, and re-entrancy guard", () => {
  const storeSrc = fs.readFileSync(path.join(ROOT, "src/lib/fonts/store.ts"), "utf8");
  assert.doesNotMatch(
    storeSrc,
    /for\s*\(\s*const\s+font\s+of\s+fonts\s*\)\s*void\s+removeUploadFromDisk/,
    "clearLocalFonts must not fire N parallel removeUploadFromDisk IPC calls",
  );
  assert.doesNotMatch(
    storeSrc,
    /for\s*\(\s*const\s+font\s+of\s+prev\s*\)\s*void\s+uninstallFontOnSystem/,
    "resetLibrary must not fire N parallel uninstallFontOnSystem IPC calls",
  );
  assert.match(storeSrc, /removeUploadsBatchFromDisk/);
  assert.match(storeSrc, /cleanupLocalFacesChunked/);
  assert.match(storeSrc, /cancelDownloadQueue\(\)/);

  const dialogSrc = fs.readFileSync(
    path.join(ROOT, "src/components/font-studio/uploads-reset-dialog.tsx"),
    "utf8",
  );
  assert.match(dialogSrc, /fm-dialog-footer mt-5/);
  assert.match(dialogSrc, /setBusy\("reset"\)/);
  assert.match(dialogSrc, /setBusy\("clear"\)/);

  const osActSrc = fs.readFileSync(path.join(ROOT, "src/lib/fonts/os-activate.ts"), "utf8");
  assert.match(osActSrc, /Permanently deleted/);
  assert.match(osActSrc, /export async function removeUploadsBatchFromDisk/);
});

test("Watch Folder smart filtering & saved rules", () => {
  const root = "C:\\Users\\Eric\\Dropbox\\MyFonts";
  const paths = [
    `${root}\\Inter\\Inter-VariableFont_opsz,wght.ttf`,
    `${root}\\Inter\\static\\Inter_18pt-Regular.ttf`,
    `${root}\\Inter\\static\\Inter_18pt-Bold.ttf`,
    `${root}\\Inter\\Inter-Regular.woff2`,
  ];
  const rule = {
    varStaticMode: "variable",
    includeVariable: true,
    includeStatic: true,
    includeItalic: true,
    formats: { ttf: true, otf: true, ttc: true, woff2: false, woff: false },
    autoActivate: true,
  };
  const kept = filterOriginPathsByRule(paths, root, rule, []);
  assert.deepEqual(kept, [`${root}\\Inter\\Inter-VariableFont_opsz,wght.ttf`]);

  // Web-only batch does not deadlock to 0 even when saved rule disables woff2.
  const webAnalysis = analyzeImportBatch([{ name: "OnlyWeb-Regular.woff2", size: 22_000 }], []);
  const merged = selectionFromSavedRule(webAnalysis, rule);
  assert.equal(merged.formats.woff2, true);
});

test("Google Download Cancel, Emoji GDI fallback, Fast Reopen & Watch Folder differentiation contracts", () => {
  const toggleSrc = fs.readFileSync(
    path.join(ROOT, "src/components/font-studio/activate-toggle.tsx"),
    "utf8",
  );
  assert.match(toggleSrc, /getActivateCancelEpoch\(\)/);

  const osActSrc = fs.readFileSync(path.join(ROOT, "src/lib/fonts/os-activate.ts"), "utf8");
  assert.match(osActSrc, /export function getActivateCancelEpoch/);
  assert.match(osActSrc, /if \(myBatch !== batchId\) return;/);

  const actRs = fs.readFileSync(path.join(ROOT, "src-tauri/src/activate.rs"), "utf8");
  assert.match(actRs, /active_queue:\s*Mutex<Option<Arc<Mutex<VecDeque<String>>>>>/);
  assert.match(actRs, /fetch_google_css_font\(client, "Noto Emoji", UA_DESKTOP_TTF, "wght@400"\)/);
  assert.match(actRs, /clear_session_gdi_refused\(family\);/);
  assert.match(actRs, /std::thread::scope\(\|s\|/);

  const mainRs = fs.readFileSync(path.join(ROOT, "src-tauri/src/lib.rs"), "utf8");
  assert.match(mainRs, /wait_for_relaunch_parent_pid/);
  assert.match(mainRs, /--relaunch-after-pid/);
  assert.match(mainRs, /handle\.exit\(0\)/);

  const watchSrc = fs.readFileSync(path.join(ROOT, "src/lib/fonts/watch-folder.ts"), "utf8");
  assert.match(watchSrc, /mode:\s*"watch"/);
  assert.match(watchSrc, /filterOriginPathsByRule/);

  const treeSrc = fs.readFileSync(path.join(ROOT, "src/components/font-studio/folder-tree.tsx"), "utf8");
  assert.match(treeSrc, /Watched Folders/);
});

test("Activate All zero pre-download & Emoji WindowsCompatible priority & scan-index fast boot contracts", () => {
  const toggleSrc = fs.readFileSync(
    path.join(ROOT, "src/components/font-studio/activate-toggle.tsx"),
    "utf8",
  );
  // Must NOT pre-enqueue wave0 before requestActivateConfirm resolves.
  const confirmIdx = toggleSrc.indexOf("requestActivateConfirm(");
  assert.ok(confirmIdx > 0);
  const beforeConfirm = toggleSrc.slice(toggleSrc.indexOf("if (usable.length > 50)"), confirmIdx);
  assert.doesNotMatch(beforeConfirm, /activateInWaves/);

  const dlgSrc = fs.readFileSync(
    path.join(ROOT, "src/components/font-studio/activate-confirm-dialog.tsx"),
    "utf8",
  );
  assert.match(dlgSrc, /Nothing starts downloading or activating until you confirm below/);
  assert.match(dlgSrc, /if \(!next && req\) resolveActivateConfirm\("abort"\)/);

  const actRs = fs.readFileSync(path.join(ROOT, "src-tauri/src/activate.rs"), "utf8");
  // Both WindowsCompatible mirrors must come BEFORE CBDT-only NotoColorEmoji.ttf.
  const nceIdx = actRs.indexOf('if slug == "noto-color-emoji"');
  const nceBlock = actRs.slice(nceIdx, nceIdx + 700);
  const rawCompatIdx = nceBlock.indexOf("raw.githubusercontent.com/googlefonts/noto-emoji/main/2D/fonts/NotoColorEmoji_WindowsCompatible.ttf");
  const cbdtIdx = nceBlock.indexOf("2D/fonts/NotoColorEmoji.ttf");
  assert.ok(rawCompatIdx > 0 && cbdtIdx > 0 && rawCompatIdx < cbdtIdx, "WindowsCompatible raw mirror must precede CBDT-only URL");
  assert.match(actRs, /fn scan_index_cached_dir_ready/);
  assert.match(actRs, /const FACE_STREAM_SLOTS: usize = 4;/);
});
