import "./lib-ts-hooks.mjs";
import assert from "node:assert/strict";
import test from "node:test";
import { readRustModule, readTsModule } from "./lib/read-source.mjs";

const { absorbReadyNames } = await import("../src/lib/fonts/ready-batch.ts");

test("first names and a full poll replace the list", () => {
  assert.deepEqual(absorbReadyNames([], ["A"]), ["A"]);
  assert.deepEqual(absorbReadyNames(["A", "B"], ["A", "B", "C"]), ["A", "B", "C"]);
});

test("a short event tail appends and does not drop earlier families", () => {
  assert.deepEqual(absorbReadyNames(["A", "B"], ["C"]), ["A", "B", "C"]);
  assert.deepEqual(absorbReadyNames(["A", "B", "C"], ["C"]), ["A", "B", "C"]);
  assert.deepEqual(absorbReadyNames(["A"], []), ["A"]);
});

test("catalog probes cannot use the 300s body timeout, and events send only the new names", () => {
  const rust = readRustModule(new URL("../src-tauri/src/activate.rs", import.meta.url));
  assert.match(rust, /const CATALOG_PROBE_TIMEOUT: Duration = Duration::from_secs\(20\)/);
  assert.match(rust, /fn ready_emit_delta/);
  // c260: probes go through the shared limiter with the same 20 s timeout.
  assert.match(rust, /\.timeout\(CATALOG_PROBE_TIMEOUT\)|timeout: Some\(CATALOG_PROBE_TIMEOUT\)/);
  assert.match(rust, /note_catalog_current/);
  const js = readTsModule(new URL("../src/lib/fonts/os-activate.ts", import.meta.url));
  assert.match(js, /absorbReadyNames/);
  assert.match(js, /return eventsBound \? 2500 : 400/);
});
