/**
 * Permanent 2-launch Windows WebView2 CDP E2E smoke test (`npm run test:desktop`).
 *
 * Runs against the sandbox debug executable (`src-tauri/target/debug/font-manager.exe`
 * built with `--config src-tauri/tauri.sandbox.conf.json`) in a strictly isolated
 * `FM_SANDBOX_DOCS` temporary directory so real user Documents, installed Font Manager,
 * and `C:\Windows\Fonts` are never touched.
 */

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, readdirSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const exePath = join(root, "src-tauri", "target", "debug", "fonths.exe");
const figtreeFixture = join(root, "tests", "fixtures", "Figtree-wght.ttf");
const interFixture = join(root, "tests", "fixtures", "Inter-opsz-wght.ttf");

if (!existsSync(exePath)) {
  console.error(
    `Missing debug sandbox binary: ${exePath}\nBuild it first: .\\node_modules\\.bin\\tauri.cmd build --debug --no-bundle --config src-tauri/tauri.sandbox.conf.json`,
  );
  process.exit(1);
}

function freePort() {
  return new Promise((res, rej) => {
    const s = createServer();
    s.once("error", rej);
    s.listen(0, "127.0.0.1", () => {
      const addr = s.address();
      s.close(() => res(addr.port));
    });
  });
}

async function waitForCdp(port, ms = 30_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return await r.json();
    } catch {
      /* waiting */
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`CDP endpoint did not come up on port ${port}`);
}

function sendWmClose(pid) {
  const ps = `
$targetPid = ${pid}
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class FmDesktopE2eClose {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
}
"@
[FmDesktopE2eClose]::EnumWindows({
  param($hWnd, $lParam)
  [uint32]$wpid = 0
  [void][FmDesktopE2eClose]::GetWindowThreadProcessId($hWnd, [ref]$wpid)
  if ($wpid -eq $targetPid) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][FmDesktopE2eClose]::GetWindowTextW($hWnd, $sb, 256)
    if ($sb.ToString() -like "*Fonths*" -or $sb.ToString() -like "*Font Manager*") {
      [void][FmDesktopE2eClose]::PostMessageW($hWnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    }
  }
  return $true
}, [IntPtr]::Zero) | Out-Null
`;
  spawnSync("powershell.exe", ["-NoProfile", "-Command", ps], { encoding: "utf8" });
}

function waitExit(child, ms = 15_000) {
  return new Promise((res) => {
    if (child.exitCode !== null) return res({ exitCode: child.exitCode, timedOut: false });
    const t = setTimeout(() => res({ exitCode: child.exitCode, timedOut: true }), ms);
    child.once("exit", (code) => {
      clearTimeout(t);
      res({ exitCode: code, timedOut: false });
    });
  });
}

async function launchApp(tmpRoot, port) {
  const wvDir = join(tmpRoot, "WebView2");
  mkdirSync(wvDir, { recursive: true });
  const env = {
    ...process.env,
    FM_SANDBOX_DOCS: tmpRoot,
    WEBVIEW2_USER_DATA_FOLDER: wvDir,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
  };
  const child = spawn(exePath, [], { cwd: root, env, stdio: "pipe" });
  await waitForCdp(port, 35_000);
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let page = null;
  const end = Date.now() + 20_000;
  while (Date.now() < end && !page) {
    for (const ctx of browser.contexts()) {
      for (const p of ctx.pages()) {
        if (p.url() && !p.url().startsWith("devtools://")) {
          page = p;
          break;
        }
      }
      if (page) break;
    }
    if (!page) await new Promise((r) => setTimeout(r, 200));
  }
  assert.ok(page, "WebView2 page not found over CDP");
  const pageErrors = [];
  page.on("pageerror", (err) => pageErrors.push(String(err)));
  await page.waitForSelector("article.fm-font-card", { timeout: 30_000 });
  return { child, browser, page, pageErrors };
}

const tmpRoot = join(os.tmpdir(), `fm-desktop-e2e-${Date.now()}`);
mkdirSync(tmpRoot, { recursive: true });
let child1 = null;
let child2 = null;

try {
  // Run 1
  const r1 = await launchApp(tmpRoot, await freePort());
  child1 = r1.child;
  const p1 = r1.page;

  const dw = await p1.evaluate(() => window.__TAURI_INTERNALS__.invoke("dwrite_engine_info"));
  assert.equal(dw.engine, "DirectWrite");
  assert.equal(typeof dw.elevated, "boolean");
  assert.ok(dw.systemFontCount > 0);

  // Google Fonts -> ... -> Activate all must NOT pre-download before confirm; Cancel starts nothing
  const gfontsRow = p1.locator('[aria-label="Google Fonts"]').locator("..");
  await gfontsRow.hover();
  await gfontsRow.locator('button[aria-label="Google Fonts menu"]').click();
  await p1.waitForSelector('[data-testid="activate-all"]', { timeout: 5_000 });
  await p1.click('[data-testid="activate-all"]');
  await p1.waitForSelector('[data-testid="activate-confirm-dialog"]', { timeout: 5_000 });
  await p1.waitForTimeout(300);
  const preConfirmProg = await p1.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("google_download_progress"),
  );
  assert.equal(preConfirmProg.running, false, "Activate All must NOT start downloading before user confirms");
  assert.equal(preConfirmProg.total, 0, "Activate All must NOT queue any fonts before user confirms");
  await p1.click('[data-testid="activate-confirm-cancel"]');
  await p1.waitForTimeout(300);
  const afterCancelProg = await p1.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("google_download_progress"),
  );
  assert.equal(afterCancelProg.running, false, "Cancel on Activate All confirm must not start downloads");
  assert.equal(afterCancelProg.total, 0, "Cancel on Activate All confirm must leave queue empty");

  // Settings + Separated Fast GDI Restore + Delete Mode + Clean Temp Cache + Purge non-Google
  await p1.click('[data-testid="settings-open"]');
  await p1.waitForSelector('[data-testid="delete-mode-card"]', { timeout: 10_000 });
  await p1.waitForSelector('[data-testid="gdi-restore-card"]', { timeout: 5_000 });
  await p1.waitForSelector('[data-testid="purge-non-google-btn"]', { timeout: 5_000 });
  await p1.click('[data-testid="delete-mode-permanent"]');
  await p1.click('[data-testid="clean-temp-cache-btn"]');
  await p1.waitForTimeout(300);
  await p1.keyboard.press("Escape");

  // Notification Center Bell icon recorded the temp-clean toast & uses scrollable list
  await p1.click('[data-testid="notifications-open"]');
  await p1.waitForSelector('[data-testid="notifications-popover"]', { timeout: 5_000 });
  const notifInfo = await p1.evaluate(() => {
    const pop = document.querySelector('[data-testid="notifications-popover"]');
    const list = document.querySelector('[data-testid="notifications-list"]');
    return {
      text: pop?.textContent ?? "",
      overflowY: list ? getComputedStyle(list).overflowY : "",
    };
  });
  assert.ok(notifInfo.text.includes("Temp cache") || notifInfo.text.includes("temporary"), `Expected recorded toast in Notification Center, got: ${notifInfo.text}`);
  assert.equal(notifInfo.overflowY, "auto", `Expected scrollable notifications list, got overflowY=${notifInfo.overflowY}`);
  await p1.keyboard.press("Escape");

  // Playground live Ink color + OpenType feature toggles + scrollable/collapsible top controls
  await p1.getByRole("link", { name: "Playground" }).click();
  await p1.waitForSelector('[data-testid="playground-fg-color"]', { timeout: 10_000 });
  await p1.waitForSelector('[data-testid="playground-top-controls"]', { timeout: 5_000 });
  await p1.click('[data-testid="playground-toggle-controls"]');
  await p1.waitForTimeout(150);
  const collapsed = await p1.locator('[data-testid="playground-top-controls"]').count();
  assert.equal(collapsed, 0, "Expected top controls to collapse when Focus viewport clicked");
  await p1.click('[data-testid="playground-toggle-controls"]');
  await p1.waitForSelector('[data-testid="playground-top-controls"]', { timeout: 5_000 });
  await p1.waitForSelector('[data-testid="lock-heading-btn"]', { timeout: 5_000 });
  await p1.waitForSelector('[data-testid="lock-body-btn"]', { timeout: 5_000 });
  await p1.click('[data-testid="lock-heading-btn"]');
  const headingLocked = await p1.locator('[data-testid="lock-heading-btn"]').getAttribute("aria-pressed");
  assert.equal(headingLocked, "true", "Expected lock-heading-btn aria-pressed=true after click");
  await p1.click('[data-testid="suggest-pair-btn"]');
  await p1.waitForTimeout(150);
  await p1.click('[data-testid="lock-heading-btn"]');
  await p1.locator('[data-testid="playground-fg-color"]').fill("#224488");
  await p1.locator('[data-testid="playground-ot-smcp"]').click();
  await p1.waitForTimeout(200);
  const vpStyle = await p1.locator('[data-testid="playground-viewport-primary"]').evaluate((el) => ({
    color: el.style.color,
    ot: el.querySelector("textarea")?.style.fontFeatureSettings || "",
    fvs: el.querySelector("textarea")?.style.fontVariationSettings || "",
  }));
  assert.ok(vpStyle.color.includes("34, 68, 136") || vpStyle.color === "#224488");
  assert.ok(vpStyle.ot.includes("smcp"));

  // Local uploads + Inspector Permanent Delete prompt
  await p1.getByRole("link", { name: "Library" }).click();
  await p1.locator('[data-testid="font-file-input"]').setInputFiles([figtreeFixture, interFixture]);
  await p1.locator('[aria-label="Local Files"]').click();
  const figCard = p1.locator('article.fm-font-card[data-font-card-family="Figtree"]').first();
  const interCard = p1.locator('article.fm-font-card[data-font-card-family="Inter"]').first();
  await figCard.locator('button[aria-label="Deactivate"]').waitFor({ state: "visible", timeout: 20_000 });
  await interCard.locator('button[aria-label="Deactivate"]').waitFor({ state: "visible", timeout: 20_000 });

  // Right-click desktop context menu on Font Card
  await figCard.click({ button: "right" });
  await p1.waitForSelector('[data-testid="app-context-menu"]', { timeout: 5_000 });
  const menuText = await p1.locator('[data-testid="app-context-menu"]').innerText();
  assert.ok(menuText.includes("Copy Family Name") && menuText.includes("Open Inspector"), `Unexpected context menu: ${menuText}`);
  await p1.keyboard.press("Escape");
  await p1.waitForTimeout(150);

  // Duplicates tab detects uploaded Figtree & Inter matching Google catalog
  await p1.getByRole("link", { name: "Duplicates" }).click();
  await p1.waitForFunction(() => document.body.innerText.includes("Figtree") && document.body.innerText.includes("Same family"), { timeout: 10_000 });
  await p1.waitForSelector('[data-testid="auto-hide-duplicates-switch"]', { timeout: 5_000 });
  await p1.getByRole("link", { name: "Library" }).click();
  await p1.locator('[aria-label="Local Files"]').click();
  await figCard.locator("p.fm-spec").first().click();
  await p1.waitForSelector("aside.fm-inspector", { timeout: 10_000 });
  await p1.locator('aside.fm-inspector button[aria-label="Delete font"], aside.fm-inspector button:has-text("Delete")').first().click();
  await p1.waitForSelector('[data-testid="inspector-delete-dialog"]', { timeout: 10_000 });
  const delText = await p1.locator('[data-testid="inspector-delete-dialog"]').innerText();
  assert.ok(delText.includes("Permanently Delete"));
  await p1.click('[data-testid="inspector-delete-cancel"]');

  const uploadsRoot = join(tmpRoot, "Documents", "Fonths", "Uploads");
  const uploadDirs = existsSync(uploadsRoot) ? readdirSync(uploadsRoot) : [];
  assert.ok(uploadDirs.includes("Figtree") && uploadDirs.includes("Inter"));

  await p1.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("set_desktop_prefs", {
      closeToTray: false,
      startWithWindows: false,
      preferStaticFaces: false,
      fastSessionResume: false,
      deleteMode: "permanent",
    }),
  );
  await p1.waitForTimeout(600);
  assert.equal(r1.pageErrors.length, 0, `Run 1 pageErrors: ${r1.pageErrors.join("; ")}`);
  await r1.browser.close();
  sendWmClose(r1.child.pid);
  const ex1 = await waitExit(r1.child, 15_000);
  assert.equal(!ex1.timedOut && ex1.exitCode === 0, true);

  await new Promise((r) => setTimeout(r, 700));

  // Run 2: Verify persistence across exit & reopen
  const r2 = await launchApp(tmpRoot, await freePort());
  child2 = r2.child;
  const p2 = r2.page;

  let restored = null;
  const settleEnd = Date.now() + 20_000;
  while (Date.now() < settleEnd) {
    restored = await p2.evaluate(() => {
      const raw = window.localStorage.getItem("font-manager:v1");
      const parsed = raw ? JSON.parse(raw) : null;
      return {
        activated: parsed?.state?.activated ?? [],
        deleteMode: parsed?.state?.desktopPrefs?.deleteMode ?? null,
      };
    });
    if (restored.activated.filter((id) => id.startsWith("l-")).length >= 2) break;
    await p2.waitForTimeout(400);
  }
  assert.equal(restored.activated.filter((id) => id.startsWith("l-")).length, 2);
  assert.equal(restored.deleteMode, "permanent");

  // Smart Local Import dialog: dropping static Inter-Regular.ttf when Variable Inter is already in library
  const staticInterCopy = join(tmpRoot, "Inter-Regular.ttf");
  const { copyFileSync } = await import("node:fs");
  copyFileSync(interFixture, staticInterCopy);
  await p2.locator('[data-testid="font-file-input"]').setInputFiles([staticInterCopy]);
  await p2.waitForSelector('[data-testid="import-options-dialog"]', { timeout: 10_000 });
  const importDlgText = await p2.locator('[data-testid="import-options-dialog"]').innerText();
  assert.ok(
    importDlgText.includes("Variable & Static") && importDlgText.includes("Inter"),
    `Expected Variable vs Static prompt for Inter, got: ${importDlgText}`,
  );
  await p2.waitForSelector('[data-testid="import-var-mode-variable"]', { timeout: 5_000 });
  await p2.waitForSelector('[data-testid="import-var-mode-static"]', { timeout: 5_000 });
  await p2.waitForSelector('[data-testid="import-fmt-ttf"]', { timeout: 5_000 });
  await p2.waitForSelector('[data-testid="import-auto-activate"]', { timeout: 5_000 });
  await p2.keyboard.press("Escape");
  await p2.waitForTimeout(200);

  assert.equal(r2.pageErrors.length, 0, `Run 2 pageErrors: ${r2.pageErrors.join("; ")}`);

  await r2.browser.close();
  sendWmClose(r2.child.pid);
  const ex2 = await waitExit(r2.child, 15_000);
  assert.equal(!ex2.timedOut && ex2.exitCode === 0, true);

  console.log("desktop-cdp.e2e.mjs: PASS (DirectWrite, Notification Center, Playground, Smart Deletion, Smart Import, 2-launch persistence)");
} finally {
  if (child1 && child1.exitCode === null) try { process.kill(child1.pid); } catch { /* ignore */ }
  if (child2 && child2.exitCode === null) try { process.kill(child2.pid); } catch { /* ignore */ }
  for (let i = 0; i < 6; i++) {
    try { rmSync(tmpRoot, { recursive: true, force: true }); break; } catch { await new Promise((r) => setTimeout(r, 400)); }
  }
}
