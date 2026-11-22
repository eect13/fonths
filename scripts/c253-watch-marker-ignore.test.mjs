import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { isOwnMarkerOnlyEvent } from "../src/lib/fonts/watch-event-filter.ts";

// C253 #1: marker writes (.complete/.expected/.google-planned...) inside the managed
// root re-kicked the scan every ~6 s. Marker-only events must be ignored.
test("c253: marker-only watch events are ignored", () => {
  const root = "C:\\Users\\Eric\\Documents\\Font Manager\\Gidugu";
  assert.equal(isOwnMarkerOnlyEvent({ paths: [`${root}\\.complete`, `${root}\\.expected`] }), true);
  assert.equal(isOwnMarkerOnlyEvent({ paths: ["/x/Font Manager/Roboto/.google-planned"] }), true);
});

test("c253: real font changes still trigger a sync", () => {
  assert.equal(isOwnMarkerOnlyEvent({ paths: ["C:\\fm\\Gidugu\\gidugu-400-normal.ttf"] }), false);
  assert.equal(isOwnMarkerOnlyEvent({ paths: ["C:\\fm\\Gidugu\\.complete", "C:\\fm\\New\\a.otf"] }), false);
  assert.equal(isOwnMarkerOnlyEvent({ paths: ["C:\\fm\\NewFamily"] }), false);
  assert.equal(isOwnMarkerOnlyEvent({ paths: [] }), false);
  assert.equal(isOwnMarkerOnlyEvent(undefined), false);
});

test("c253: managed-root watch uses the marker filter", () => {
  const src = readFileSync(new URL("../src/lib/fonts/watch-folder.ts", import.meta.url), "utf8");
  assert.match(src, /managedUnwatch = await watch\(root, \(ev\) => \{\s*if \(isOwnMarkerOnlyEvent\(ev\)\) return;/);
});
