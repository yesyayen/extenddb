// Headless-browser test for the Vectors tab (vector index + SearchVectors).
//
// Serves crates/wasm/web/ and drives it in a real headless Chromium: the
// seeded Music table carries a COSINE vector index over 8-d embeddings, the
// Vectors tab creates a fresh vector-indexed table from the form, items are
// inserted through the CLI shell, SearchVectors runs from the panel (both a
// picked item and a typed vector) and renders ranked results with scores,
// the data browser shows the vector index pill and truncated vector previews,
// the raw JSON and CLI consoles round-trip SearchVectors, and the browser
// observes ZERO network requests after ready.
//
// Run:  cd crates/wasm/tests-browser && npm install && node vector.spec.mjs

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

function fail(msg) { console.error("VECTOR TEST FAILED: " + msg); process.exit(1); }
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

const entryCount = (page) => page.locator('[data-testid="log-entry"]').count();
async function waitNewEntry(page, before) {
  await page.waitForFunction(
    (n) => document.querySelectorAll('[data-testid="log-entry"]').length > n,
    before, { timeout: 10000 }
  );
}
async function newestEntry(page) {
  const e = page.locator('[data-testid="log-entry"]').first();
  return { text: await e.innerText(), cls: (await e.getAttribute("class")) || "" };
}
// Run a CLI command through the shell and return the newest log entry.
async function cli(page, cmd) {
  await page.locator('[data-testid="tab-cli"]').click();
  await page.locator('[data-testid="cli"]').fill(cmd);
  const before = await entryCount(page);
  await page.locator('[data-testid="cli-run"]').click();
  await waitNewEntry(page, before);
  return newestEntry(page);
}
async function resultRows(page) {
  const rows = page.locator('[data-testid="vec-result-row"]');
  const n = await rows.count();
  const out = [];
  for (let i = 0; i < n; i++) {
    const cells = await rows.nth(i).locator("td").allInnerTexts();
    out.push(cells); // [rank, score, ...attrs]
  }
  return out;
}
function assertAscendingScores(rows, label) {
  const scores = rows.map((r) => Number(r[1]));
  for (const s of scores) assert(Number.isFinite(s), label + ": non-numeric score in " + JSON.stringify(rows));
  for (let i = 1; i < scores.length; i++) {
    assert(scores[i] >= scores[i - 1], label + ": scores not ascending: " + JSON.stringify(scores));
  }
  return scores;
}

async function main() {
  const server = await startServer();
  const port = server.address().port;
  const url = `http://127.0.0.1:${port}/index.html`;

  const browser = await chromium.launch();
  const page = await browser.newPage();

  let readyReached = false;
  const postReadyRequests = [];
  page.on("request", (req) => { if (readyReached) postReadyRequests.push(req.url()); });
  page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));

  await page.goto(url, { waitUntil: "load" });
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
  readyReached = true;
  console.log("  [0] engine ready (body[data-ready])");

  // 1) The seed carries the vector index: data browser pill for Music.
  const pill = page.locator('[data-testid="tbl-vector"]');
  await page.waitForFunction(() => {
    const p = document.querySelector('[data-testid="tbl-vector"]');
    return p && !p.hidden && p.textContent.includes("vidx");
  }, { timeout: 10000 });
  const pillText = await pill.innerText();
  assert(pillText.includes("COSINE") && pillText.includes("8d"),
    "vector pill missing metric/dims: " + pillText);
  console.log("  [1] data browser pill shows the seeded vector index: " + pillText);

  // 2) Vector attributes render as truncated previews in the grid, not walls
  //    of floats: exactly 3 leading dims, an ellipsis, and the dim count.
  const gridText = await page.locator('[data-testid="grid"]').innerText();
  assert(/\[[-0-9.]+, [-0-9.]+, [-0-9.]+, \u2026\] \(8d\)/.test(gridText),
    "grid does not truncate the emb vector: " + gridText.slice(0, 400));
  console.log("  [2] grid renders emb as a truncated preview '[a, b, c, \u2026] (8d)'");

  // 3) Vectors tab: search the seeded Music index by picking an existing item.
  await page.locator('[data-testid="tab-vec"]').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="vec-item"] option').length > 1, { timeout: 10000 });
  assert((await page.locator('[data-testid="vec-table"]').inputValue()) === "Music",
    "Music not selected in the vector table picker");
  const idxLabel = await page.locator('[data-testid="vec-index"] option:checked').innerText();
  assert(idxLabel.includes("vidx") && idxLabel.includes("COSINE") && idxLabel.includes("8d"),
    "index picker label wrong: " + idxLabel);
  // pick "Radiohead · Paranoid Android" as the query item
  const itemLabel = "Radiohead \u00b7 Paranoid Android";
  await page.locator('[data-testid="vec-item"]').selectOption({ label: itemLabel });
  const q = await page.locator('[data-testid="vec-query"]').inputValue();
  assert(q.startsWith("[0.9, 0.2, 0.3"), "picking an item did not fill the query vector: " + q);
  await page.locator('[data-testid="vec-k"]').fill("5");
  let before = await entryCount(page);
  await page.locator('[data-testid="vec-run"]').click();
  await waitNewEntry(page, before);
  let entry = await newestEntry(page);
  assert(entry.text.includes("SearchVectors") && entry.text.includes("200"),
    "SearchVectors log entry missing/failed: " + entry.text.slice(0, 300));
  let rows = await resultRows(page);
  assert(rows.length === 5, "expected 5 ranked rows, got " + rows.length);
  const scores = assertAscendingScores(rows, "Music search");
  assert(scores[0] === 0, "self-match should score 0.0000, got " + scores[0]);
  const flat = rows.map((r) => r.join(" ")).join("\n");
  assert(flat.includes("Paranoid Android"), "top result should be the queried item:\n" + flat);
  // The engine omits the vector attribute from SearchVectors results unless a
  // ProjectionExpression asks for it (parity-verified against native sqlite),
  // so ranked rows are naturally float-wall-free.
  assert(!flat.includes("(8d)"), "vector attribute unexpectedly returned by default:\n" + flat);
  console.log("  [3] pick-an-item search: 5 ranked results, ascending scores, self-match first (score 0)");
  console.log("      top-3: " + rows.slice(0, 3).map((r) => `#${r[0]} ${r[1]} ${r.slice(2, 4).join(" / ")}`).join(" | "));

  // 4) Full UI flow on a fresh table: create (form) -> insert (CLI) -> search
  //    (typed vector) -> ranked results.
  await page.locator('[data-testid="tab-vec"]').click();
  await page.locator('[data-testid="vec-new-table"]').fill("Colors");
  await page.locator('[data-testid="vec-new-index"]').fill("cidx");
  await page.locator('[data-testid="vec-new-attr"]').fill("rgb");
  await page.locator('[data-testid="vec-new-dims"]').fill("3");
  await page.locator('[data-testid="vec-new-metric"]').selectOption("EUCLIDEAN");
  before = await entryCount(page);
  await page.locator('[data-testid="vec-create"]').click();
  await waitNewEntry(page, before);
  entry = await newestEntry(page);
  assert(entry.text.includes("CreateTable") && entry.text.includes("200") && !entry.cls.includes("err"),
    "vector CreateTable failed: " + entry.text.slice(0, 300));
  await page.waitForFunction(() => {
    const s = document.querySelector('[data-testid="vec-table"]');
    return s && [...s.options].some((o) => o.value === "Colors");
  }, { timeout: 10000 });
  console.log("  [4] created table Colors with vector index cidx (EUCLIDEAN, 3d) from the form");

  // insert three colors through the CLI shell
  for (const [name, r, g, b] of [["red", 1, 0, 0], ["yellow", 1, 1, 0], ["blue", 0, 0, 1]]) {
    const put = await cli(page,
      `aws dynamodb put-item --table-name Colors --item '{"pk":{"S":"${name}"},"rgb":{"L":[{"N":"${r}"},{"N":"${g}"},{"N":"${b}"}]}}'`);
    assert(put.text.includes("200") && !put.cls.includes("err"), "put-item " + name + " failed: " + put.text.slice(0, 300));
  }
  console.log("  [5] inserted red/yellow/blue via the CLI shell");

  // search with a typed query vector: orange = [1, 0.6, 0]
  await page.locator('[data-testid="tab-vec"]').click();
  await page.locator('[data-testid="vec-table"]').selectOption("Colors");
  await page.locator('[data-testid="vec-query"]').fill("[1, 0.6, 0]");
  await page.locator('[data-testid="vec-k"]').fill("10");
  before = await entryCount(page);
  await page.locator('[data-testid="vec-run"]').click();
  await waitNewEntry(page, before);
  rows = await resultRows(page);
  assert(rows.length === 3, "TopK 10 over 3 items should return 3 rows, got " + rows.length);
  assertAscendingScores(rows, "Colors search");
  const order = rows.map((r) => r[2]);
  assert(JSON.stringify(order) === JSON.stringify(["yellow", "red", "blue"]),
    "euclidean ranking wrong for orange query: " + JSON.stringify(order));
  console.log("  [6] typed-vector search on Colors ranks yellow < red < blue for 'orange' (TopK > item count OK)");

  // 5) Raw JSON console: the SearchVectors sample round-trips.
  await page.locator('[data-testid="tab-raw"]').click();
  await page.locator("#sample-buttons button", { hasText: "SearchVectors" }).first().click();
  assert((await page.locator("#target").inputValue()) === "DynamoDB_20120810.SearchVectors",
    "SearchVectors sample did not load the target");
  before = await entryCount(page);
  await page.locator('[data-testid="run"]').click();
  await waitNewEntry(page, before);
  entry = await newestEntry(page);
  assert(entry.text.includes("SearchResults") && entry.text.includes("Score") && !entry.cls.includes("err"),
    "raw SearchVectors failed: " + entry.text.slice(0, 300));
  console.log("  [7] raw JSON SearchVectors sample round-trips (SearchResults + Score)");

  // 6) CLI shell: search-vectors kebab op round-trips.
  const cliOut = await cli(page,
    "aws dynamodb search-vectors --table-name Colors --index-name cidx " +
    "--search-vector '[{\"N\":\"1\"},{\"N\":\"0.6\"},{\"N\":\"0\"}]' --top-k 2");
  assert(cliOut.text.includes("SearchVectors") && cliOut.text.includes("SearchResults") && !cliOut.cls.includes("err"),
    "CLI search-vectors failed: " + cliOut.text.slice(0, 300));
  assert(cliOut.text.indexOf("yellow") < cliOut.text.indexOf("red"),
    "CLI search-vectors ranking wrong: " + cliOut.text.slice(0, 400));
  console.log("  [8] CLI shell search-vectors round-trips with correct ranking");

  // 7) Zero network after ready (browser-observed).
  assert(postReadyRequests.length === 0,
    "browser observed network after ready: " + JSON.stringify(postReadyRequests));
  console.log("  [9] zero network after ready: the whole vector flow ran in-tab");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("VECTOR TEST PASSED: create -> insert -> search -> ranked results, browser pill + truncation, raw/CLI parity, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
