// Headless-browser test for the Vectors tab (vector index + SearchVectors).
//
// Serves crates/wasm/web/ and drives it in a real headless Chromium: the
// seeded Music table carries a COSINE vector index over 8-d embeddings, the
// Vectors subsection is form-driven with a shared table/index context bar and
// collapsible sections (create / put with text / search) whose open state
// persists across reloads, the old pick-an-item query dropdown is gone, grid
// vectors render as truncating chips with expand/copy, a fresh vector-indexed
// table is created from the form, items are inserted through the CLI shell,
// SearchVectors runs from the advanced raw-vector input and renders ranked
// results with scores and a source label, the raw JSON and CLI consoles
// round-trip SearchVectors, and the browser observes ZERO network requests
// after ready.
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

  // 2) Vector attributes render as truncating chips in the grid, not walls
  //    of floats: 3 leading dims, an ellipsis, the dim count, and expand/copy.
  const gridChips = page.locator('[data-testid="grid"] [data-testid="vec-chip"]');
  assert((await gridChips.count()) > 0, "grid renders no vector chips");
  const chipText = await gridChips.first().innerText();
  assert(/\[-?\d+\.\d{3}, -?\d+\.\d{3}, -?\d+\.\d{3}, \u2026\] \u00b7 8d/.test(chipText),
    "grid chip does not truncate the emb vector: " + chipText);
  // expand: the full JSON appears in a fixed-height scrollable box, then collapses.
  await page.locator('[data-testid="grid"] [data-testid="vec-chip-expand"]').first().click();
  const fullBox = page.locator('[data-testid="grid"] [data-testid="vec-full"]');
  assert((await fullBox.count()) === 1, "chip expand did not open the full-JSON box");
  const fullVec = JSON.parse(await fullBox.innerText());
  assert(Array.isArray(fullVec) && fullVec.length === 8 && fullVec.every(Number.isFinite),
    "expanded chip JSON is not the full 8-d vector: " + (await fullBox.innerText()).slice(0, 120));
  await page.locator('[data-testid="grid"] [data-testid="vec-chip-expand"]').first().click();
  assert((await fullBox.count()) === 0, "chip expand did not collapse again");
  console.log("  [2] grid renders emb as a chip '" + chipText + "' with working expand/collapse");

  // 3) Vectors tab: context bar selects Music/vidx; the item picker is GONE;
  //    search the seeded index through the advanced raw-vector input.
  await page.locator('[data-testid="tab-vec"]').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="vec-table"] option').length >= 1, { timeout: 10000 });
  assert((await page.locator('[data-testid="vec-item"]').count()) === 0,
    "the pick-an-item query dropdown should be removed");
  assert((await page.locator('[data-testid="vec-table"]').inputValue()) === "Music",
    "Music not selected in the vector table picker");
  const idxLabel = await page.locator('[data-testid="vec-index"] option:checked').innerText();
  assert(idxLabel.includes("vidx") && idxLabel.includes("COSINE") && idxLabel.includes("8d"),
    "index picker label wrong: " + idxLabel);
  // default section states: search open, create/put/advanced-raw closed.
  assert(await page.locator('[data-testid="vec-sec-search"]').evaluate((d) => d.open), "search section should default open");
  assert(!(await page.locator('[data-testid="vec-sec-create"]').evaluate((d) => d.open)), "create section should default closed");
  assert(!(await page.locator('[data-testid="vec-sec-put"]').evaluate((d) => d.open)), "put section should default closed");
  assert(!(await page.locator('[data-testid="vec-sec-raw"]').evaluate((d) => d.open)), "raw input should default closed");
  await page.locator('[data-testid="vec-sec-raw"] > summary').click();
  await page.locator('[data-testid="vec-query"]').fill("[0.9, 0.2, 0.3, 0, 0.7, 0.4, 0.3, 0.62]");
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
  assert((await page.locator('[data-testid="vec-result-row"] [data-testid="vec-chip"]').count()) === 0,
    "vector attribute unexpectedly returned by default:\n" + flat);
  const label = await page.locator('[data-testid="vec-results-label"]').innerText();
  assert(label.includes("raw vector") && label.includes("Music/vidx"),
    "results label does not name the raw-vector input: " + label);
  console.log("  [3] raw-vector search: item picker gone, 5 ranked results, ascending scores, self-match first, labeled '" + label + "'");

  // 4) Full UI flow on a fresh table: create (collapsed form section) ->
  //    insert (CLI) -> search (raw vector) -> ranked results.
  await page.locator('[data-testid="tab-vec"]').click();
  await page.locator('[data-testid="vec-sec-create"] > summary').click();
  assert(await page.locator('[data-testid="vec-sec-create"]').evaluate((d) => d.open), "create section did not open");
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

  // search with a raw query vector: orange = [1, 0.6, 0] (the advanced input
  // is still open from step 3; its state persisted in this session).
  await page.locator('[data-testid="tab-vec"]').click();
  await page.locator('[data-testid="vec-table"]').selectOption("Colors");
  assert(await page.locator('[data-testid="vec-sec-raw"]').evaluate((d) => d.open),
    "advanced raw input should still be open (persisted within the session)");
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
  console.log("  [6] raw-vector search on Colors ranks yellow < red < blue for 'orange' (TopK > item count OK)");

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

  // 8) Section open/closed state persists across a reload (localStorage):
  //    create + advanced-raw were opened above, put stayed closed.
  readyReached = false; // the reload's own fetches are expected
  await page.reload({ waitUntil: "load" });
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
  await page.locator('[data-testid="tab-vec"]').click();
  for (const [sec, want] of [["vec-sec-create", true], ["vec-sec-raw", true], ["vec-sec-put", false], ["vec-sec-search", true]]) {
    const open = await page.locator(`[data-testid="${sec}"]`).evaluate((d) => d.open);
    assert(open === want, `${sec} open state not persisted across reload: got ${open}, want ${want}`);
  }
  console.log("  [10] section open/closed state persisted across reload (create/raw open, put closed, search open)");

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("VECTOR TEST PASSED: collapsible sections + persistence, item picker removed, vector chips, create -> insert -> search -> labeled ranked results, raw/CLI parity, zero network");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
