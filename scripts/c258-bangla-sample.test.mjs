// C258 P3 (Skye 256 D): Tiro Bangla's card showed the Latin pangram — "Bangla"
// was not mapped to the Bengali script, so the Bengali sample never applied.
import "./lib-ts-hooks.mjs";
import assert from "node:assert/strict";
import test from "node:test";

const { scriptOf, scriptSampleText } = await import("../src/lib/fonts/scripts.ts");

test("c258: Tiro Bangla / Anek Bangla preview the Bengali sample", () => {
  for (const fam of ["Tiro Bangla", "Anek Bangla"]) {
    assert.equal(scriptOf(fam), "bengali", fam);
    assert.equal(scriptSampleText(fam), "হ্যালো বিশ্ব", fam);
  }
  assert.equal(scriptOf("Tiro Devanagari Hindi"), "devanagari");
  assert.equal(scriptOf("Barlow"), "latin");
});
