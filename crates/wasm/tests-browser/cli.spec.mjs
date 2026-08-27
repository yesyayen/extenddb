// Headless-browser test for the ExtendDB browser playground (U2 CLI shell).
//
// Drives the AWS CLI-style shell in a real headless Chromium: the parser maps
// `aws dynamodb <op> --flags` to the wire call, round-trips through the wasm
// engine, renders a boxed timestamped log entry, reflects the equivalent
// X-Amz-Target/JSON into the raw console, handles control-plane AND data-plane
// ops, and issues no network.
//
// Run:  cd crates/wasm/tests-browser && npm install && node cli.spec.mjs

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

function fail(msg) { console.error("CLI TEST FAILED: " + msg); process.exit(1); }
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

// Type a CLI command, run it, wait for a new boxed log entry, return log text.
async function cli(page, cmd) {
  await page.fill('[data-testid="cli"]', cmd);
  const before = await page.locator('[data-testid="log-entry"]').count();
  await page.locator('[data-testid="cli-run"]').click();
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 10000 }
  );
  return page.locator('[data-testid="log"]').innerText();
}
// The newest log entry (an error box has class err).
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
  await page.locator('[data-testid="tab-cli"]').click(); // CLI is default, be explicit
  console.log("  [0] engine ready, CLI tab active, page pre-seeded");

  // 1) list-tables: kebab op -> ListTables, Music present, wire call reflected.
  let log = await cli(page, "aws dynamodb list-tables");
  assert(log.includes("ListTables"), "list-tables op not in log");
  assert(log.includes("Music"), "list-tables missing Music");
  assert((await page.inputValue("#target")) === "DynamoDB_20120810.ListTables",
    "list-tables did not reflect the wire target");
  console.log("  [1] list-tables -> ListTables, Music present, wire call reflected");

  // 2) query with a quoted key-condition + JSON expression-attribute-values.
  log = await cli(page,
    'aws dynamodb query --table-name Music --key-condition-expression "Artist = :a" --expression-attribute-values \'{":a":{"S":"Radiohead"}}\'');
  assert(log.includes("Paranoid Android") && log.includes("Karma Police"),
    "query via CLI missing seeded Radiohead items");
  const qBody = JSON.parse(await page.inputValue("#body"));
  assert(qBody.KeyConditionExpression === "Artist = :a", "query KeyConditionExpression wrong");
  assert(qBody.ExpressionAttributeValues[":a"].S === "Radiohead", "query EAV not parsed as JSON");
  console.log("  [2] query --flags parsed (quoted expr + JSON EAV), seeded items returned");

  // 3) put-item then get-item round-trips a CLI-written item.
  await cli(page,
    'aws dynamodb put-item --table-name Music --item \'{"Artist":{"S":"Muse"},"SongTitle":{"S":"Hysteria"},"Year":{"N":"2003"}}\'');
  log = await cli(page,
    'aws dynamodb get-item --table-name Music --key \'{"Artist":{"S":"Muse"},"SongTitle":{"S":"Hysteria"}}\'');
  assert(log.includes("Hysteria") && log.includes("2003"), "get-item did not return the CLI-written item");
  console.log("  [3] put-item then get-item round-trips a CLI-written item");

  // 4) scan --limit 2 : numeric flag coercion.
  log = await cli(page, "aws dynamodb scan --table-name Music --limit 2");
  const sBody = JSON.parse(await page.inputValue("#body"));
  assert(sBody.Limit === 2 && typeof sBody.Limit === "number", "scan --limit not coerced to number");
  assert(/"Count":\s*2/.test(log), "scan --limit 2 did not cap Count");
  console.log("  [4] scan --limit 2 coerced to a numeric Limit and capped results");

  // 5) parse error surfaces as an error entry without dispatching.
  await cli(page, "aws dynamodb query --expression-attribute-values {bad json");
  assert(await newest(page).evaluate((e) => e.classList.contains("err")), "bad JSON not an error entry");
  assert((await newest(page).innerText()).includes("parse error"), "bad JSON did not surface parse error");
  console.log("  [5] bad --expression-attribute-values JSON -> error entry (no dispatch)");

  // 6) --flag=value form (botocore-style) is accepted.
  log = await cli(page,
    'aws dynamodb get-item --table-name=Music --key \'{"Artist":{"S":"Radiohead"},"SongTitle":{"S":"Paranoid Android"}}\'');
  assert(log.includes("OK Computer"), "--flag=value form did not resolve (get-item missed)");
  assert((await page.inputValue("#target")) === "DynamoDB_20120810.GetItem", "equals-form target wrong");
  console.log("  [6] --flag=value form parses (get-item --table-name=Music)");

  // 7) non-numeric --limit is rejected (error entry, not silently null).
  await cli(page, "aws dynamodb scan --table-name Music --limit abc");
  assert(await newest(page).evaluate((e) => e.classList.contains("err")), "non-numeric --limit not rejected");
  console.log("  [7] non-numeric --limit surfaces a parse error");

  // 8) CONTROL PLANE via the CLI: create-table then list-tables shows it.
  log = await cli(page,
    'aws dynamodb create-table --table-name Demo --attribute-definitions \'[{"AttributeName":"pk","AttributeType":"S"}]\' --key-schema \'[{"AttributeName":"pk","KeyType":"HASH"}]\' --billing-mode PAY_PER_REQUEST');
  assert(!(await newest(page).evaluate((e) => e.classList.contains("err"))), "create-table via CLI errored");
  log = await cli(page, "aws dynamodb list-tables");
  assert(log.includes("Demo"), "created Demo table not listed (CLI control plane broken)");
  console.log("  [8] control-plane via CLI: create-table then list-tables shows Demo");

  // 9) everything above ran in-tab: zero network after ready.
  assert(postReadyRequests.length === 0,
    "browser observed network after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [9] zero network after ready: the CLI shell dispatches in-tab");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("CLI TEST PASSED: aws dynamodb <op> --flags (control + data plane) maps to the wire call, round-trips, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
