// Headless-browser test for the ExtendDB browser playground (batch operations).
//
// Drives BatchWriteItem and BatchGetItem through all three client tabs the way a
// user does: load the sample, run it, read the log. The CLI path proves that
// --request-items is JSON-decoded, the data browser proves the write landed (two
// new rows, one seeded row gone) without a manual refresh, the Raw JSON and JS SDK
// samples prove the same requests in the other two dialects, and nothing goes
// over the network after the engine is ready.
//
// Run:  cd crates/wasm/tests-browser && npm install && node batch.spec.mjs

import http from "node:http";
import { deepStrictEqual } from "node:assert";
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

function fail(msg) { console.error("BATCH TEST FAILED: " + msg); process.exit(1); }
function assert(cond, msg) { if (!cond) fail(msg); }
function assertDeep(actual, expected, msg) {
  try { deepStrictEqual(actual, expected); }
  catch (e) { fail(msg + "\n" + e.message); }
}

async function startServer() {
  const wasmPath = path.join(WEB_DIR, "pkg", "extenddb_wasm_bg.wasm");
  try { await stat(wasmPath); }
  catch {
    fail("no web build found at " + wasmPath +
      "\n  build it first: scripts/wasm-build.sh --target web --dev --out-dir web/pkg");
  }
  const sdkPath = path.join(WEB_DIR, "pkg-sdk", "extenddb-sdk.js");
  try { await stat(sdkPath); }
  catch {
    fail("no SDK bundle found at " + sdkPath +
      "\n  build it first: cd crates/wasm/web-sdk && npm install && npm run build");
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

// Click a run button and wait for one new boxed log entry.
async function runAndWait(page, runTestId) {
  const before = await page.locator('[data-testid="log-entry"]').count();
  await page.locator(`[data-testid="${runTestId}"]`).click();
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 15000 }
  );
}
// The newest log entry (an error box has class err). Assertions read only its
// response box, parsed as JSON, so the echoed request can never satisfy them
// and every check is on exact shape, not substrings.
const newest = (page) => page.locator('[data-testid="log-entry"]').first();
const newestIsErr = (page) => newest(page).evaluate((e) => e.classList.contains("err"));
async function newestRespJson(page) {
  const text = await newest(page).locator(".log-body.resp .log-body-text").evaluate((e) => e.textContent);
  try { return JSON.parse(text); } catch (e) { fail("newest response box is not JSON: " + e.message + "\n" + text); }
}
// CLI and Raw entries wrap the wire response as { statusCode, response }; the
// SDK entry holds the snippet's return value directly.
const wireResp = async (page) => (await newestRespJson(page)).response;
// After a sample button click, the textarea shows its first line and is focused.
async function assertLoadedTopDown(page, selector, where) {
  const st = await page.locator(selector).evaluate((el) => ({
    sel: el.selectionStart, top: el.scrollTop, focused: document.activeElement === el,
  }));
  assert(st.sel === 0 && st.top === 0 && st.focused,
    `${where}: sample did not load top-down and focused: ${JSON.stringify(st)}`);
}
const gridRows = (page, n) => page.waitForFunction(
  (want) => document.querySelectorAll('[data-testid="grid"] tbody tr').length === want,
  n, { timeout: 10000 }
);

const NEW_TITLES = ["Foundation and Empire", "Second Foundation"];
const DELETED_TITLE = "I, Robot";
// Title -> Year of the three seeded keys the get sample reads.
const GET_ITEMS = { "Foundation": "1951", "Pride and Prejudice": "1813", "Murder on the Orient Express": "1934" };
const PROJECTED_KEYS = ["Genre", "Title", "Year"];
const WRITE_CAPACITY = [{ CapacityUnits: 3, TableName: "Books" }];

// The write response: nothing unprocessed, exactly three write units on Books.
function assertBatchWriteResp(resp, where) {
  assertDeep(resp.UnprocessedItems, {}, `${where}: BatchWriteItem UnprocessedItems is not {}`);
  assertDeep(resp.ConsumedCapacity, WRITE_CAPACITY, `${where}: BatchWriteItem ConsumedCapacity is not 3 units on Books`);
}
// The three projected items: exactly Genre / Title / Year each, the right Years.
function assertBatchGetItems(items, where) {
  assert(Array.isArray(items) && items.length === 3, `${where}: expected three items, got ${JSON.stringify(items)}`);
  const got = {};
  for (const it of items) {
    assertDeep(Object.keys(it).sort(), PROJECTED_KEYS, `${where}: ProjectionExpression not applied exactly`);
    assert(typeof it.Genre.S === "string", `${where}: Genre is not a string attribute`);
    got[it.Title.S] = it.Year.N;
  }
  assertDeep(got, GET_ITEMS, `${where}: BatchGetItem returned the wrong titles or years`);
}
// The full wire response of the get: Responses.Books plus nothing unprocessed,
// and no ConsumedCapacity because none was requested.
function assertBatchGetResp(resp, where) {
  assertDeep(Object.keys(resp).sort(), ["Responses", "UnprocessedKeys"], `${where}: BatchGetItem response has unexpected members`);
  assertDeep(Object.keys(resp.Responses), ["Books"], `${where}: BatchGetItem Responses is not exactly Books`);
  assertBatchGetItems(resp.Responses.Books, where);
  assertDeep(resp.UnprocessedKeys, {}, `${where}: BatchGetItem UnprocessedKeys is not {}`);
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
  await page.locator('[data-testid="tab-cli"]').click();
  await gridRows(page, 30);
  let gridText = await page.locator('[data-testid="grid"]').innerText();
  assert(gridText.includes(DELETED_TITLE), `seeded grid missing ${DELETED_TITLE} (the sample's delete target)`);
  console.log("  [0] engine ready, CLI tab active, Books grid pre-seeded (30 rows)");

  // 1) CLI batch-write-item sample: --request-items is decoded as JSON and dispatched.
  await page.locator('[data-testid="cli-sample-batch-write-item"]').click();
  const writeCmd = await page.inputValue('[data-testid="cli"]');
  assert(writeCmd.startsWith("aws dynamodb batch-write-item"), "batch-write-item sample did not load into the CLI");
  await assertLoadedTopDown(page, '[data-testid="cli"]', "CLI batch-write-item");
  await runAndWait(page, "cli-run");
  assert(!(await newestIsErr(page)), "batch-write-item errored: " + (await newest(page).innerText()));
  assert((await page.inputValue("#target")) === "DynamoDB_20120810.BatchWriteItem",
    "batch-write-item did not reflect the BatchWriteItem wire target");
  const wBody = JSON.parse(await page.inputValue("#body"));
  assert(Array.isArray(wBody.RequestItems && wBody.RequestItems.Books) && wBody.RequestItems.Books.length === 3,
    "--request-items was not JSON-decoded into RequestItems.Books[3]");
  assert(wBody.RequestItems.Books.filter((r) => r.PutRequest).length === 2 &&
    wBody.RequestItems.Books.filter((r) => r.DeleteRequest).length === 1,
    "batch-write-item sample is not two PutRequests plus one DeleteRequest");
  assertBatchWriteResp(await wireResp(page), "CLI");
  console.log("  [1] CLI batch-write-item: --request-items JSON-decoded, 2 puts + 1 delete, exact response shape");

  // 2) The data browser reflects the batch write with no manual refresh: 30 + 2 - 1.
  await gridRows(page, 31);
  gridText = await page.locator('[data-testid="grid"]').innerText();
  for (const t of NEW_TITLES) assert(gridText.includes(t), `grid missing batch-written row ${t}`);
  assert(!gridText.includes(DELETED_TITLE), `grid still shows the batch-deleted row ${DELETED_TITLE}`);
  const count = (await page.locator('[data-testid="tbl-count"]').innerText()).trim();
  assert(count === "31 items", `count badge after batch write wrong: "${count}"`);
  console.log("  [2] data browser: two new rows present, deleted row gone, 31 items, no manual refresh");

  // 3) CLI batch-get-item sample: three keys, projection with an aliased reserved word.
  await page.locator('[data-testid="cli-sample-batch-get-item"]').click();
  await assertLoadedTopDown(page, '[data-testid="cli"]', "CLI batch-get-item");
  await runAndWait(page, "cli-run");
  assert(!(await newestIsErr(page)), "batch-get-item errored: " + (await newest(page).innerText()));
  assert((await page.inputValue("#target")) === "DynamoDB_20120810.BatchGetItem",
    "batch-get-item did not reflect the BatchGetItem wire target");
  const gBody = JSON.parse(await page.inputValue("#body"));
  const books = gBody.RequestItems && gBody.RequestItems.Books;
  assert(books && Array.isArray(books.Keys) && books.Keys.length === 3, "batch-get-item sample is not three Books keys");
  assert(typeof books.ProjectionExpression === "string" && books.ExpressionAttributeNames["#y"] === "Year",
    "batch-get-item sample lacks the ProjectionExpression + ExpressionAttributeNames pair");
  assertBatchGetResp(await wireResp(page), "CLI");
  console.log("  [3] CLI batch-get-item: exactly three items with Genre / Title / Year, UnprocessedKeys {}");

  // 4) Raw JSON samples: the same two requests as X-Amz-Target + body. The
  //    parsed bodies must equal what the CLI dialect produced (no drift).
  await page.locator('[data-testid="tab-raw"]').click();
  await page.waitForSelector('[data-testid="panel-raw"]:not([hidden])', { timeout: 5000 });
  await page.locator('[data-testid="raw-sample-BatchGetItem"]').click();
  assert((await page.inputValue("#target")) === "DynamoDB_20120810.BatchGetItem", "Raw BatchGetItem sample target wrong");
  await assertLoadedTopDown(page, "#body", "Raw BatchGetItem");
  assertDeep(JSON.parse(await page.inputValue("#body")), gBody, "Raw BatchGetItem body differs from the CLI dialect");
  await runAndWait(page, "run");
  assert(!(await newestIsErr(page)), "Raw BatchGetItem errored: " + (await newest(page).innerText()));
  assertBatchGetResp(await wireResp(page), "Raw JSON");
  await page.locator('[data-testid="raw-sample-BatchWriteItem"]').click();
  assert((await page.inputValue("#target")) === "DynamoDB_20120810.BatchWriteItem", "Raw BatchWriteItem sample target wrong");
  await assertLoadedTopDown(page, "#body", "Raw BatchWriteItem");
  assertDeep(JSON.parse(await page.inputValue("#body")), wBody, "Raw BatchWriteItem body differs from the CLI dialect");
  await runAndWait(page, "run");
  assert(!(await newestIsErr(page)), "Raw BatchWriteItem errored: " + (await newest(page).innerText()));
  assertBatchWriteResp(await wireResp(page), "Raw JSON");
  // Re-running the write is idempotent: the puts overwrite, the delete is a no-op.
  await gridRows(page, 31);
  console.log("  [4] Raw JSON samples equal the CLI bodies and run with the same results; the grid stays at 31");

  // 5) JS SDK samples: BatchWriteItemCommand and BatchGetItemCommand through the real SDK.
  await page.locator('[data-testid="tab-sdk"]').click();
  await page.waitForSelector('[data-testid="panel-sdk"]:not([hidden])', { timeout: 5000 });
  const hasCmds = await page.evaluate(() =>
    !!(window.ExtendDBSDK && window.ExtendDBSDK.BatchWriteItemCommand && window.ExtendDBSDK.BatchGetItemCommand));
  assert(hasCmds, "window.ExtendDBSDK lacks BatchWriteItemCommand / BatchGetItemCommand");
  await page.locator('[data-testid="sdk-sample-BatchWriteItem"]').click();
  assert((await page.inputValue('[data-testid="sdk"]')).includes("new BatchWriteItemCommand("),
    "SDK BatchWriteItem sample did not load");
  await assertLoadedTopDown(page, '[data-testid="sdk"]', "SDK BatchWriteItem");
  await runAndWait(page, "sdk-run");
  assert(!(await newestIsErr(page)), "SDK BatchWriteItem errored: " + (await newest(page).innerText()));
  // The snippet returns { UnprocessedItems, ConsumedCapacity } from the SDK output.
  assertBatchWriteResp(await newestRespJson(page), "JS SDK");
  await page.locator('[data-testid="sdk-sample-BatchGetItem"]').click();
  assert((await page.inputValue('[data-testid="sdk"]')).includes("new BatchGetItemCommand("),
    "SDK BatchGetItem sample did not load");
  await assertLoadedTopDown(page, '[data-testid="sdk"]', "SDK BatchGetItem");
  await runAndWait(page, "sdk-run");
  assert(!(await newestIsErr(page)), "SDK BatchGetItem errored: " + (await newest(page).innerText()));
  // The snippet returns out.Responses.Books: the item list itself.
  assertBatchGetItems(await newestRespJson(page), "JS SDK");
  await gridRows(page, 31);
  console.log("  [5] JS SDK BatchWriteItemCommand and BatchGetItemCommand samples run through the real SDK");

  // 6) Everything above ran in-tab.
  assert(postReadyRequests.length === 0,
    "browser observed network after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [6] zero network after ready: batch operations dispatch in-tab");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("BATCH TEST PASSED: BatchWriteItem and BatchGetItem samples run from the CLI, Raw JSON, and JS SDK tabs; the data browser reflects the write; zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
