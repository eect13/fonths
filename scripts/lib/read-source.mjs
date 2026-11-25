import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function resolveCandidate(pathOrUrl) {
  if (pathOrUrl instanceof URL) return fileURLToPath(pathOrUrl);
  return String(pathOrUrl);
}

/**
 * Read a Rust module source or concatenate all `.rs` files in its modular folder
 * (e.g. `src-tauri/src/activate.rs` -> `src-tauri/src/activate/*.rs`).
 */
export function readRustModule(rootOrPath, maybeRelPath, options = {}) {
  const target = maybeRelPath
    ? join(rootOrPath, maybeRelPath)
    : resolveCandidate(rootOrPath);
  const excludeTests = Boolean(options.excludeTests);

  if (existsSync(target) && statSync(target).isFile()) {
    return readFileSync(target, "utf8");
  }

  const modDir = target.endsWith(".rs") ? target.slice(0, -3) : target;
  if (!existsSync(modDir) || !statSync(modDir).isDirectory()) {
    return readFileSync(target, "utf8");
  }

  const files = readdirSync(modDir)
    .filter((f) => f.endsWith(".rs") && (!excludeTests || f !== "tests.rs"))
    .sort((a, b) => {
      if (a === "mod.rs") return -1;
      if (b === "mod.rs") return 1;
      if (a === "tests.rs") return 1;
      if (b === "tests.rs") return -1;
      return a.localeCompare(b);
    });

  return files.map((f) => readFileSync(join(modDir, f), "utf8")).join("\n");
}

/**
 * Read a TypeScript module file together with any extracted submodule directory
 * (e.g. `src/lib/fonts/store.ts` + `src/lib/fonts/store/*.ts`, or
 * `src/lib/fonts/os-activate.ts` + `src/lib/fonts/activation/*.ts`).
 */
export function readTsModule(rootOrPath, maybeRelPath) {
  const target = maybeRelPath
    ? join(rootOrPath, maybeRelPath)
    : resolveCandidate(rootOrPath);

  const chunks = [];
  if (existsSync(target) && statSync(target).isFile()) {
    chunks.push(readFileSync(target, "utf8"));
  }

  const extraDirs = [];
  if (target.endsWith(".ts")) {
    extraDirs.push(target.slice(0, -3));
    if (target.endsWith("os-activate.ts")) {
      extraDirs.push(join(dirname(target), "activation"));
    }
  } else {
    extraDirs.push(target);
  }

  for (const dir of extraDirs) {
    if (!existsSync(dir) || !statSync(dir).isDirectory()) continue;
    const subFiles = readdirSync(dir)
      .filter((f) => f.endsWith(".ts") || f.endsWith(".tsx"))
      .sort((a, b) => a.localeCompare(b));
    for (const f of subFiles) {
      chunks.push(readFileSync(join(dir, f), "utf8"));
    }
  }

  if (!chunks.length) {
    return readFileSync(target, "utf8");
  }
  return chunks.join("\n");
}
