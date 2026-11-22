import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const dialog = readFileSync(new URL("../src/components/font-studio/activate-confirm-dialog.tsx", import.meta.url), "utf8");
const toggle = readFileSync(new URL("../src/components/font-studio/activate-toggle.tsx", import.meta.url), "utf8");

test("dismissing the Activate All dialog aborts cleanly (zero pre-download without user go)", () => {
  const m = dialog.match(/onOpenChange=\{\(next\) => \{([\s\S]*?)\}\}/);
  assert.ok(m, "dialog onOpenChange handler");
  assert.match(m[1], /resolveActivateConfirm\("abort"\)/);
  assert.doesNotMatch(m[1], /resolveActivateConfirm\("ok"\)/);
});

test("OK queues ordered families only after explicit confirmation", () => {
  assert.match(toggle, /if \(choice === "ok"\) \{\s*void activateInWaves\(ordered, label\);/);
  assert.doesNotMatch(toggle, /if \(prefer\.length\) \{\s*void activateInWaves\(prefer/);
});
