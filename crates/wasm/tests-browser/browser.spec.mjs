// Headless-browser test for the ExtendDB browser playground (U4 data browser).
//
// Drives the table selector + live item grid in a real headless Chromium: the
// grid renders the pre-seeded Music table (30 tracks), a write shows up as a
// new row without a manual refresh, and everything runs in-tab (zero network).
//
// Run:  cd crates/wasm/tests-browser && node browser.spec.mjs

import http from "node:http";
import { readFile, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { chromium } from "playwright";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const WEB_DIR = path.resolve(__dirname, "..", "web");

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".css": "text/css; charset=utf-8",
  ".map": "application/json",
};

function fail(msg) { console.error("BROWSER TEST FAILED: " + msg); process.exit(1); }
function assert(cond, msg) { if (!cond) fail(msg); }

async function startServer() {
  const wasmPath = path.join(WEB_DIR, "pkg", "extenddb_wasm_bg.wasm");
  try { await stat(wasmPath); }
  catch {
    fail("no web build found at " + wasmPath +
      "\n  build it first: scripts/wasm-build.sh --target web --dev --out-dir web/pkg");
  }
  const server = http.createServer(async (req, res) => {
    try {
      const urlPath = decodeURIComponent(req.url.split("?")[0]);
      const rel = urlPath === "/" ? "/index.html" : urlPath;
      const filePath = path.join(WEB_DIR, path.normalize(rel));
      if (!filePath.startsWith(WEB_DIR)) { res.writeHead(403).end(); return; }
      const buf = await readFile(filePath);
      const ext = path.extname(filePath).toLowerCase();
      res.writeHead(200, { "content-type": MIME[ext] || "application/octet-stream" });
      res.end(buf);
    } catch { res.writeHead(404).end("not found"); }
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  return server;
}

async function main() {
  const server = await startServer();
  const port = server.address().port;
  const browser = await chromium.launch();
  const page = await browser.newPage();

  let readyReached = false;
  const postReadyRequests = [];
  page.on("request", (req) => { if (readyReached) postReadyRequests.push(req.url()); });
  page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));

  await page.goto(`http://127.0.0.1:${port}/index.html`, { waitUntil: "load" });
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
  readyReached = true;

  // 1) On boot the grid renders the pre-seeded Music table (30 rows).
  await page.waitForFunction(
    () => document.querySelectorAll('[data-testid="grid"] tbody tr').length === 30,
    { timeout: 10000 }
  );
  let gridText = await page.locator('[data-testid="grid"]').innerText();
  assert(gridText.includes("Radiohead") && gridText.includes("Paranoid Android"),
    "grid missing seeded Music rows");
  assert(/Artist \(partition key\)/.test(gridText) && /SongTitle \(sort key\)/.test(gridText),
    "grid did not label partition/sort key columns");
  const count0 = (await page.locator('[data-testid="tbl-count"]').innerText()).trim();
  assert(count0 === "30 items", `count badge wrong: "${count0}"`);
  console.log("  [1] grid renders the pre-seeded Music table (30 rows, key columns marked)");

  // 2) A write appears as a new row without a manual refresh (via Raw console).
  await page.locator('[data-testid="tab-raw"]').click();
  await page.evaluate(() => {
    document.getElementById("target").value = "DynamoDB_20120810.PutItem";
    document.getElementById("body").value = JSON.stringify({
      TableName: "Music",
      Item: { Artist: { S: "Blur" }, SongTitle: { S: "Song 2" }, Year: { N: "1997" } },
    });
  });
  await page.locator('[data-testid="run"]').click();
  await page.waitForFunction(
    () => document.querySelectorAll('[data-testid="grid"] tbody tr').length === 31,
    { timeout: 10000 }
  );
  gridText = await page.locator('[data-testid="grid"]').innerText();
  assert(gridText.includes("Blur") && gridText.includes("Song 2"),
    "written item did not appear in the grid");
  const count1 = (await page.locator('[data-testid="tbl-count"]').innerText()).trim();
  assert(count1 === "31 items", `count badge after write wrong: "${count1}"`);
  console.log("  [2] a PutItem shows up as a new grid row with no manual refresh (30 -> 31)");

  // 3) Refresh re-scans in-tab; everything ran with zero network.
  await page.locator('[data-testid="tbl-refresh"]').click();
  await page.waitForFunction(
    () => document.querySelectorAll('[data-testid="grid"] tbody tr').length === 31,
    { timeout: 10000 }
  );
  assert(postReadyRequests.length === 0,
    "browser observed network after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [3] zero network after ready: the data browser scans in-tab");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("BROWSER TEST PASSED: live item grid pre-seeded (30), updates on write, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
