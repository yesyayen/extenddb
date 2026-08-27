// Headless-browser test for the ExtendDB browser playground (U3 SDK console).
//
// Switches to the JS SDK tab, loads the bundled real @aws-sdk/client-dynamodb
// (window.ExtendDBSDK), runs SDK code in the page against the wasm engine, and
// asserts a typed exception on a failed condition, the grid cap on a bulk
// insert, and zero network.
//
// Prereqs (beyond U1/U2): cd crates/wasm/web-sdk && npm install && npm run build
//
// Run:  cd crates/wasm/tests-browser && node sdk.spec.mjs

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

function fail(msg) { console.error("SDK-UI TEST FAILED: " + msg); process.exit(1); }
function assert(cond, msg) { if (!cond) fail(msg); }

async function startServer() {
  for (const p of ["pkg/extenddb_wasm_bg.wasm", "pkg-sdk/extenddb-sdk.js"]) {
    try { await stat(path.join(WEB_DIR, p)); }
    catch {
      fail("missing build artifact: " + path.join(WEB_DIR, p) +
        (p.includes("pkg-sdk")
          ? "\n  build it: cd crates/wasm/web-sdk && npm install && npm run build"
          : "\n  build it: scripts/wasm-build.sh --target web --dev --out-dir web/pkg"));
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

// Put code in the SDK console, run it, wait for a new boxed entry, return log.
async function runSdkSnippet(page, code) {
  await page.fill('[data-testid="sdk"]', code);
  const before = await page.locator('[data-testid="log-entry"]').count();
  await page.locator('[data-testid="sdk-run"]').click();
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 15000 }
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
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
  readyReached = true;

  // Switch to the JS SDK tab (hidden by default).
  await page.locator('[data-testid="tab-sdk"]').click();
  await page.waitForSelector('[data-testid="panel-sdk"]:not([hidden])', { timeout: 5000 });

  // 0) The real SDK bundle actually loaded and the client is wired.
  const hasSdk = await page.evaluate(() => !!(window.ExtendDBSDK && window.ExtendDBSDK.DynamoDBClient));
  assert(hasSdk, "window.ExtendDBSDK.DynamoDBClient not present (SDK bundle failed to load)");
  assert(!(await page.locator('[data-testid="sdk-run"]').isDisabled()), "SDK Run disabled (bundle missing?)");
  console.log("  [0] real @aws-sdk/client-dynamodb loaded, client wired, JS SDK tab active");

  // 1) Default example: a real QueryCommand round-trips through the engine.
  let log = await runSdkSnippet(page, await page.inputValue('[data-testid="sdk"]'));
  assert(!(await newest(page).evaluate((e) => e.classList.contains("err"))), "SDK Query errored");
  assert(log.includes("Paranoid Android") && log.includes("Karma Police"),
    "SDK Query missing seeded Radiohead items");
  console.log("  [1] client.send(new QueryCommand(...)) returned the seeded items");

  // 2) A failed ConditionExpression surfaces the SDK's TYPED exception class.
  await runSdkSnippet(page,
    'await client.send(new PutItemCommand({' +
    ' TableName: "Music",' +
    ' Item: { Artist: { S: "Radiohead" }, SongTitle: { S: "Paranoid Android" } },' +
    ' ConditionExpression: "attribute_not_exists(Artist)" }));' +
    ' return "should not reach";');
  assert(await newest(page).evaluate((e) => e.classList.contains("err")), "failed condition not an error entry");
  assert((await newest(page).innerText()).includes("ConditionalCheckFailedException"),
    "expected the SDK's typed ConditionalCheckFailedException");
  console.log("  [2] failed ConditionExpression -> SDK throws typed ConditionalCheckFailedException");

  // 3) A write via the SDK is visible to a subsequent SDK read.
  log = await runSdkSnippet(page,
    'await client.send(new PutItemCommand({ TableName: "Music",' +
    ' Item: { Artist: { S: "Nirvana" }, SongTitle: { S: "Lithium" }, Year: { N: "1991" } } }));' +
    ' const g = await client.send(new GetItemCommand({ TableName: "Music",' +
    ' Key: { Artist: { S: "Nirvana" }, SongTitle: { S: "Lithium" } } }));' +
    ' return g.Item;');
  assert(log.includes("Lithium") && log.includes("1991"),
    "SDK PutItem was not visible to a subsequent SDK GetItem");
  console.log("  [3] SDK PutItem is visible to a subsequent SDK GetItem");

  // 4) Uncapped-render guard: a bulk insert via the SDK console caps the grid.
  await runSdkSnippet(page,
    'for (let i = 0; i < 60; i++) { await client.send(new PutItemCommand({ TableName: "Music", Item: { Artist: { S: "Bulk" }, SongTitle: { S: "t" + i } } })); } return "inserted 60";');
  await page.waitForFunction(
    () => document.querySelectorAll('[data-testid="grid"] tbody tr').length === 50,
    { timeout: 15000 }
  );
  const capCount = (await page.locator('[data-testid="tbl-count"]').innerText()).trim();
  assert(/showing 50 of \d+ items/.test(capCount), `grid not capped after bulk insert: "${capCount}"`);
  console.log("  [4] grid render capped at 50 rows on a 60+ item bulk insert (no unbounded DOM)");

  // 5) The headline: the SDK signs every request, yet ZERO network after ready.
  assert(postReadyRequests.length === 0,
    "the SDK issued network requests after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [5] zero network after ready: the real SDK ran entirely in-tab");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("SDK-UI TEST PASSED: real @aws-sdk/client-dynamodb runs in the page against the wasm engine, typed exceptions, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
