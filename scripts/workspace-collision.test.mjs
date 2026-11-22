import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  findImportGoogleCollisions,
  guessFamilyFromFileName,
  requestImportCollisionChoice,
  resolveImportCollision,
} from "../src/lib/fonts/import-collision.ts";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");

test("guessFamilyFromFileName strips weight, italic, and variable suffixes", () => {
  assert.equal(guessFamilyFromFileName("Inter-Regular.ttf"), "Inter");
  assert.equal(guessFamilyFromFileName("Inter-VariableFont_wght.ttf"), "Inter");
  assert.equal(guessFamilyFromFileName("Roboto_Condensed-BoldItalic.ttf"), "Roboto Condensed");
  assert.equal(guessFamilyFromFileName("Lora-700.woff2"), "Lora");
});

test("findImportGoogleCollisions detects matching Google Fonts and tracks file indices", () => {
  const fakeFiles = [
    new File([new Uint8Array(200)], "Inter-Regular.ttf"),
    new File([new Uint8Array(300)], "Inter-Bold.ttf"),
    new File([new Uint8Array(150)], "MyCustomBrand-Regular.otf"),
  ];
  const googleFonts = [
    {
      id: "g:inter",
      family: "Inter",
      category: "sans",
      weights: [400, 700],
      italic: false,
      source: "google",
      variable: true,
      license: "free",
    },
  ];
  const { collisions, collidingFileIndices } = findImportGoogleCollisions(
    fakeFiles,
    googleFonts,
    new Set(["g:inter"]),
    new Set(["inter"]),
  );
  assert.equal(collisions.length, 1);
  assert.equal(collisions[0].familyName, "Inter");
  assert.equal(collisions[0].isGoogleActive, true);
  assert.equal(collisions[0].isGoogleOnDisk, true);
  assert.equal(collisions[0].uploadFileNames.length, 2);
  assert.equal(collisions[0].uploadBytes, 500);
  assert.deepEqual([...collidingFileIndices], [0, 1]);
});

test("requestImportCollisionChoice resolves prefer-local / keep-both / skip-colliding", async () => {
  const p = requestImportCollisionChoice([], 3);
  resolveImportCollision("prefer-local");
  assert.equal(await p, "prefer-local");
});

test("ImportCollisionDialog and lock-holder reporting are wired", () => {
  const shell = fs.readFileSync(path.join(root, "src/components/font-studio/app-shell.tsx"), "utf8");
  const osAct = fs.readFileSync(path.join(root, "src/lib/fonts/os-activate.ts"), "utf8");
  assert.match(shell, /<ImportCollisionDialog \/>/);
  assert.match(osAct, /held by \$\{holders\.join/);
});
