// Headless-browser test for the dynein tab of the ExtendDB browser playground.
//
// Drives the real awslabs/dynein CLI (vendored, compiled to a SECOND wasm
// module) in a real headless Chromium. dynein's DynamoDB transport is routed
// to the SAME in-tab engine as the CLI / JS SDK / Raw tabs via
// set_host_dispatch(dispatch_http), so all four interfaces and the data browser
// share one database. This spec proves: dynein reads the shared seed, dynein
// writes are visible to the engine, a dynein-created table shows up in BOTH the
// data browser and the AWS CLI tab, and no network happens after load.
//
// Run:  cd crates/wasm/tests-browser && node dyn.spec.mjs

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

function fail(msg) { console.error("DYNEIN TEST FAILED: " + msg); process.exit(1); }
function assert(cond, msg) { if (!cond) fail(msg); }

async function startServer() {
  for (const rel of ["pkg/extenddb_wasm_bg.wasm", "pkg-dynein/extenddb_dynein_wasm_bg.wasm"]) {
    const p = path.join(WEB_DIR, rel);
    try { await stat(p); }
    catch {
      fail("missing web build: " + p +
        "\n  build the engine:  scripts/wasm-build.sh --target web --dev --out-dir web/pkg" +
        "\n  build dynein:      scripts/dynein-wasm-build.sh --target web --dev --out-dir web-pkg" +
        "\n                     cp -a crates/dynein-wasm/web-pkg crates/wasm/web/pkg-dynein");
    }
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

// Run one dynein command line via the dynein tab; wait for a new log entry.
async function dy(page, line) {
  await page.fill('[data-testid="dyn"]', line);
  const before = await page.locator('[data-testid="log-entry"]').count();
  await page.locator('[data-testid="dyn-run"]').click();
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 15000 }
  );
  return page.locator('[data-testid="log"]').innerText();
}
// Run one aws-cli command via the CLI tab (to prove cross-interface sharing).
async function cli(page, cmd) {
  await page.locator('[data-testid="tab-cli"]').click();
  await page.fill('[data-testid="cli"]', cmd);
  const before = await page.locator('[data-testid="log-entry"]').count();
  await page.locator('[data-testid="cli-run"]').click();
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 10000 }
  );
  return page.locator('[data-testid="log"]').innerText();
}
const newest = (page) => page.locator('[data-testid="log-entry"]').first();

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
  // dynein wasm (~24MB dev) is fetched + instantiated during boot, before ready.
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 60000 });
  readyReached = true;

  // 0) the dynein tab exists and is enabled (pkg-dynein loaded).
  const dynTab = page.locator('[data-testid="tab-dyn"]');
  assert(await dynTab.count() === 1, "dynein tab missing");
  assert(!(await dynTab.isDisabled()), "dynein tab disabled (pkg-dynein failed to load)");
  await dynTab.click();
  console.log("  [0] engine ready, dynein tab present and active");

  // 1) dynein reads the SHARED seed (Music was seeded by the main engine).
  let log = await dy(page, "dy list");
  assert(log.includes("Music"), "dynein `list` did not see the shared-seeded Music table");
  console.log("  [1] dynein `list` sees the shared-seeded Music table");

  // 2) dynein query returns seeded Radiohead rows (read path through shared engine).
  log = await dy(page, "dy -t Music query Radiohead");
  assert(log.includes("Paranoid Android") && log.includes("Karma Police"),
    "dynein query missed seeded Radiohead items");
  console.log("  [2] dynein `query Radiohead` returns seeded rows from the shared engine");

  // 3) dynein put then get round-trips through the shared engine.
  await dy(page, 'dy -t Music put Muse Hysteria --item \'{"Album":"Absolution","Year":2003}\'');
  log = await dy(page, "dy -t Music get Muse Hysteria");
  assert(log.includes("Hysteria") && log.includes("2003"), "dynein put->get did not round-trip");
  console.log("  [3] dynein `put` then `get` round-trips a dynein-written item");

  // 4) dynein control-plane write is visible in the DATA BROWSER (shared state).
  await dy(page, "dy admin create table Demo --keys pk,S sk,N");
  const inBrowser = await page.$$eval('[data-testid="tbl-select"] option', (os) => os.map((o) => o.value));
  assert(inBrowser.includes("Demo"), "dynein-created Demo table not in data browser (state not shared)");
  console.log("  [4] dynein-created `Demo` table appears in the data browser");

  // 5) and that SAME table is visible from the AWS CLI tab: one engine, all tabs.
  log = await cli(page, "aws dynamodb list-tables");
  assert(log.includes("Demo") && log.includes("Music"),
    "AWS CLI tab did not see the dynein-created Demo (interfaces not sharing one DB)");
  console.log("  [5] AWS CLI tab sees the dynein-created Demo -> all interfaces share one engine");

  // 6) an unsupported / bad dynein command surfaces as an error entry, no crash.
  await page.locator('[data-testid="tab-dyn"]').click();
  await dy(page, "dy bootstrap");
  assert(await newest(page).evaluate((e) => e.classList.contains("err")),
    "unsupported dynein command was not flagged as an error");
  console.log("  [6] unsupported dynein command surfaces a clean error entry");

  // 7) everything ran in-tab: zero network after ready.
  assert(postReadyRequests.length === 0,
    "browser observed network after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [7] zero network after ready: dynein dispatches in-tab");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("DYNEIN TEST PASSED: real awslabs/dynein CLI in wasm drives the shared in-tab engine, end to end, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
