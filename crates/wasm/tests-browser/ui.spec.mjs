// Headless-browser test for the ExtendDB browser playground (U1 demo shell).
//
// Serves crates/wasm/web/ and drives it in a real headless Chromium: the engine
// boots (body[data-ready]), the page is pre-seeded (funny note, 30 books), the
// boxed timestamped log renders ops, a non-2xx reads as an error entry, Reset
// re-seeds, the browser observes ZERO network after ready, and the engine wasm
// module is fetched exactly once at load.
//
// Run:  cd crates/wasm/tests-browser && npm install && node ui.spec.mjs

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

function fail(msg) { console.error("UI TEST FAILED: " + msg); process.exit(1); }
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

const logText = (page) => page.locator('[data-testid="log"]').innerText();
const entryCount = (page) => page.locator('[data-testid="log-entry"]').count();
async function waitNewEntry(page, before) {
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 10000 }
  );
}

async function main() {
  const server = await startServer();
  const port = server.address().port;
  const url = `http://127.0.0.1:${port}/index.html`;

  const browser = await chromium.launch();
  const page = await browser.newPage();

  let readyReached = false;
  const postReadyRequests = [];
  const allRequests = [];
  page.on("request", (req) => {
    allRequests.push(req.url());
    if (readyReached) postReadyRequests.push(req.url());
  });
  page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));

  await page.goto(url, { waitUntil: "load" });

  // 1) Engine boots -> body[data-ready] (no visible status pill anymore).
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
  console.log("  [1] engine ready (body[data-ready])");
  readyReached = true;

  // 2) Pre-seeded, never blank: the funny boot note reports the 30-book seed.
  await page.waitForFunction(
    () => document.querySelector('[data-testid="log"]').innerText.includes("Seeded 30 books"),
    { timeout: 10000 }
  );
  console.log("  [2] pre-seeded: boot note reports 'Seeded 30 books'");

  // 3) Run the pre-loaded Query in the Raw JSON console -> a boxed 200 entry.
  await page.locator('[data-testid="tab-raw"]').click();
  let before = await entryCount(page);
  await page.locator('[data-testid="run"]').click();
  await waitNewEntry(page, before);
  let log = await logText(page);
  assert(log.includes("Query"), "Query op not shown in log");
  assert(log.includes("A Wizard of Earthsea") && log.includes("The Left Hand of Darkness"),
    "Query result missing seeded Le Guin books");
  const newest = page.locator('[data-testid="log-entry"]').first();
  assert((await newest.locator(".log-status").innerText()).trim() === "200", "Query entry not status 200");
  assert(!(await newest.getAttribute("class")).includes("err"), "Query entry wrongly marked error");
  console.log("  [3] Query round-tripped as a boxed 200 entry (seeded items returned)");

  // 4) Non-2xx renders as an error entry: delete the table, then describe it.
  await page.evaluate(() => {
    document.getElementById("target").value = "DynamoDB_20120810.DeleteTable";
    document.getElementById("body").value = JSON.stringify({ TableName: "Books" });
  });
  before = await entryCount(page);
  await page.locator('[data-testid="run"]').click();
  await waitNewEntry(page, before);
  await page.evaluate(() => {
    document.getElementById("target").value = "DynamoDB_20120810.DescribeTable";
    document.getElementById("body").value = JSON.stringify({ TableName: "Books" });
  });
  before = await entryCount(page);
  await page.locator('[data-testid="run"]').click();
  await waitNewEntry(page, before);
  const errEntry = page.locator('[data-testid="log-entry"].err').first();
  assert(await errEntry.count() > 0, "no error log entry rendered");
  assert((await errEntry.innerText()).includes("ResourceNotFoundException"),
    "error entry missing ResourceNotFoundException");
  console.log("  [4] non-2xx renders as an error entry (ResourceNotFoundException)");

  // 5) Reset engine re-seeds a fresh database (note entry).
  before = await entryCount(page);
  await page.locator('[data-testid="reset"]').click();
  await waitNewEntry(page, before);
  await page.waitForFunction(
    () => document.querySelector('[data-testid="log"]').innerText.includes("re-seeded Books with 30 items"),
    { timeout: 10000 }
  );
  console.log("  [5] Reset engine re-seeded a fresh in-memory database (30 items)");

  // 6) Zero network after ready (browser-observed).
  assert(postReadyRequests.length === 0,
    "browser observed network after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [6] zero network after ready: all ops ran in-tab");

  // 7) Affirmative load-once proof: the engine wasm module is fetched exactly
  //    once, at load.
  const wasmLoads = allRequests.filter((u) => u.split("?")[0].endsWith(".wasm"));
  const uniqueWasm = [...new Set(wasmLoads.map((u) => u.split("?")[0]))];
  assert(wasmLoads.length === uniqueWasm.length,
    `a wasm module was fetched more than once: ${JSON.stringify(wasmLoads)}`);
  assert(wasmLoads.length === 1 && wasmLoads[0].split("?")[0].endsWith("/pkg/extenddb_wasm_bg.wasm"),
    `expected exactly the engine wasm at load: ${JSON.stringify(wasmLoads)}`);
  console.log("  [7] the engine wasm module fetched exactly once at load (load-once story affirmed)");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("UI TEST PASSED: demo shell boots, pre-seeds 30 books, boxed timestamped log, error entry + reset, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
