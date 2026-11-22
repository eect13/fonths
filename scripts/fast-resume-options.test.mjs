import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  coerceDesktopPrefs,
  DEFAULT_DESKTOP_PREFS,
} from "../src/lib/desktop/prefs.ts";
import {
  requestActivateConfirm,
  requestDeactivateConfirm,
  resolveActivateConfirm,
  resolveDeactivateConfirm,
  subscribeDeactivateConfirm,
} from "../src/lib/fonts/activate-confirm.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");

test("fastSessionResume pref defaults false and coerces truthy/falsy values", () => {
  assert.equal(DEFAULT_DESKTOP_PREFS.fastSessionResume, false);
  assert.equal(coerceDesktopPrefs(null).fastSessionResume, false);
  assert.equal(coerceDesktopPrefs({ fastSessionResume: 1 }).fastSessionResume, true);
  assert.equal(coerceDesktopPrefs({ fastSessionResume: false }).fastSessionResume, false);
});

test("requestDeactivateConfirm resolves google / all / keep-fast / cancel cleanly", async () => {
  const seen = [];
  const unsub = subscribeDeactivateConfirm((r) => seen.push(r));
  try {
    const p1 = requestDeactivateConfirm({
      label: "Library",
      total: 120,
      googleCount: 100,
      localCount: 20,
    });
    assert.equal(seen.at(-1)?.total, 120);
    resolveDeactivateConfirm("google");
    assert.equal(await p1, "google");

    const p2 = requestDeactivateConfirm({
      label: "Google Fonts",
      total: 200,
      googleCount: 200,
      localCount: 0,
    });
    resolveDeactivateConfirm("keep-fast");
    assert.equal(await p2, "keep-fast");

    // Replacing an open activate confirm resolves the prior prompt as abort.
    const pAct1 = requestActivateConfirm({
      label: "Google Fonts",
      total: 80,
      preferCount: 10,
      remainderCount: 70,
      visibleCount: 0,
      cancelCount: 10,
      etaHint: "~1m",
    });
    const pAct2 = requestActivateConfirm({
      label: "Library",
      total: 90,
      preferCount: 0,
      remainderCount: 90,
      visibleCount: 20,
      cancelCount: 20,
      etaHint: "~1m",
    });
    assert.equal(await pAct1, "abort");
    resolveActivateConfirm("ok");
    assert.equal(await pAct2, "ok");
  } finally {
    unsub();
  }
});

test("UI mounts DeactivateConfirmDialog, Fast Resume toggles, and bulk Deactivate logic popup", () => {
  const shell = fs.readFileSync(path.join(root, "src/components/font-studio/app-shell.tsx"), "utf8");
  const dlg = fs.readFileSync(path.join(root, "src/components/font-studio/activate-confirm-dialog.tsx"), "utf8");
  const settings = fs.readFileSync(path.join(root, "src/components/font-studio/desktop-settings.tsx"), "utf8");
  const toggle = fs.readFileSync(path.join(root, "src/components/font-studio/activate-toggle.tsx"), "utf8");

  assert.match(shell, /<DeactivateConfirmDialog \/>/);
  assert.match(dlg, /deactivate-fast-resume-toggle/);
  assert.match(dlg, /activate-fast-resume-toggle/);
  assert.match(dlg, /Google Fonts only/);
  assert.match(dlg, /Keep active/);
  assert.doesNotMatch(settings, /fast-session-resume-toggle/);
  assert.match(toggle, /export async function confirmDeactivateSet/);
  assert.match(toggle, /BULK_DEACTIVATE_CONFIRM_THRESHOLD = 50/);
});
