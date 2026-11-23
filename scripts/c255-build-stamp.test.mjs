import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const buildRs = readFileSync(new URL("../src-tauri/build.rs", import.meta.url), "utf8");
const mainRs = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");

test("c255 P3: exe embeds the git commit it was built from", () => {
  assert.match(buildRs, /cargo:rustc-env=FM_BUILD_SHA=/);
  assert.match(buildRs, /"unknown"/, "no-git builds (ZIP) must still compile");
  assert.match(mainRs, /env!\("FM_BUILD_SHA"\)/);
});
