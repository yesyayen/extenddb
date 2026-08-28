// Headless-browser test for the text -> vector semantic-search flow.
//
// Serves crates/wasm/web/ and drives it in a real headless Chromium: the
// seeded Quotes table carries real 384-d sentence embeddings (precomputed at
// build time by tools/embed-seed.mjs), the Vectors tab embeds typed query
// text in-tab with the vendored model (lazy same-origin load, visible
// status), fills the query-vector field, and returns semantically sensible
// ranked results; the in-browser embedding of a seed sentence matches the
// checked-in build-time embedding within float tolerance (seed/query model
// drift guard); "add item with text" embeds and PutItems from its collapsible
// section; the embed -> wire vector JSON helper logs a collapsed dispatch-style
// entry whose JSON pastes straight into the CLI shell; the text flow is
// pinned to the model's dimensions with a visible disable reason (other
// indexes keep the raw flow); CLI create-table still accepts arbitrary
// dimensions; and the browser observes ZERO non-same-origin network
// requests, ever.
//
// Run:  cd crates/wasm/tests-browser && npm install && node embed.spec.mjs

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
  ".onnx": "application/octet-stream",
};

function fail(msg) { console.error("EMBED TEST FAILED: " + msg); process.exit(1); }
function assert(cond, msg) { if (!cond) fail(msg); }

async function startServer() {
  for (const [rel, hint] of [
    [["pkg", "extenddb_wasm_bg.wasm"], "scripts/wasm-build.sh --target web --dev --out-dir web/pkg"],
    [["vendor", "transformers", "transformers.min.js"], "cd crates/wasm/tools && npm install && node vendor-embed-assets.mjs"],
    [["models", "Xenova", "all-MiniLM-L6-v2", "onnx", "model_quantized.onnx"], "cd crates/wasm/tools && node vendor-embed-assets.mjs"],
    [["data", "quotes-seed.json"], "cd crates/wasm/tools && node embed-seed.mjs"],
  ]) {
    const p = path.join(WEB_DIR, ...rel);
    try { await stat(p); } catch { fail("missing " + p + "\n  build it first: " + hint); }
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
    before, { timeout: 120000 }
  );
}
async function newestEntry(page) {
  const e = page.locator('[data-testid="log-entry"]').first();
  return { text: await e.innerText(), cls: (await e.getAttribute("class")) || "" };
}
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
  for (let i = 0; i < n; i++) out.push(await rows.nth(i).locator("td").allInnerTexts());
  return out; // [rank, score, ...attrs]
}

async function main() {
  const seed = JSON.parse(await readFile(path.join(WEB_DIR, "data", "quotes-seed.json"), "utf8"));
  assert(seed.model === "Xenova/all-MiniLM-L6-v2" && seed.dimensions === 384,
    "unexpected seed header: " + seed.model + " " + seed.dimensions);
  assert(seed.items.length >= 100, "seed too small: " + seed.items.length);

  const server = await startServer();
  const port = server.address().port;
  const origin = `http://127.0.0.1:${port}`;

  const browser = await chromium.launch();
  const page = await browser.newPage();

  // Every request the page ever makes must be same-origin (vendored runtime,
  // model, seed, engine). Post-ready requests are additionally recorded so
  // we can show the lazy model load stayed same-origin too.
  let readyReached = false;
  const crossOrigin = [];
  const postReady = [];
  page.on("request", (req) => {
    const u = req.url();
    if (!u.startsWith(origin + "/")) crossOrigin.push(u);
    if (readyReached) postReady.push(u);
  });
  page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));

  await page.goto(`${origin}/index.html`, { waitUntil: "load" });
  await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
  readyReached = true;
  const bootNote = await page.locator('[data-testid="log"]').innerText();
  assert(bootNote.includes(`${seed.items.length} one-liners into Quotes`),
    "boot note does not report the Quotes seed: " + bootNote.slice(0, 300));
  console.log(`  [0] engine ready; boot note reports ${seed.items.length} seeded Quotes`);

  // 1) Data browser: Quotes carries the 384-d COSINE index, vectors truncate.
  await page.locator('[data-testid="tbl-select"]').selectOption("Quotes");
  await page.waitForFunction(() => {
    const p = document.querySelector('[data-testid="tbl-vector"]');
    return p && !p.hidden && p.textContent.includes("vidx");
  }, { timeout: 10000 });
  const pillText = await page.locator('[data-testid="tbl-vector"]').innerText();
  assert(pillText.includes("COSINE") && pillText.includes("384d"), "Quotes vector pill wrong: " + pillText);
  const gridText = await page.locator('[data-testid="grid"]').innerText();
  assert(gridText.includes("\u00b7 384d"), "grid does not truncate 384-d vectors to chips: " + gridText.slice(0, 300));
  console.log("  [1] data browser: Quotes pill '" + pillText + "', 384-d vectors truncated to chips");

  // 2) Model/dimension pinning: text flow enabled for Quotes (384d = model),
  //    disabled for an 8-d index with a visible plain-text reason. The 8-d
  //    table is created through the CLI (Pins), since no seed table is 8-d.
  await cli(page,
    'aws dynamodb create-table --table-name Pins ' +
    '--attribute-definitions \'[{"AttributeName":"pk","AttributeType":"S"}]\' ' +
    '--key-schema \'[{"AttributeName":"pk","KeyType":"HASH"}]\' --billing-mode PAY_PER_REQUEST ' +
    '--vector-indexes \'[{"IndexName":"vidx8","Dimensions":8,"DistanceFunction":"COSINE",' +
    '"VectorAttribute":{"AttributeName":"emb"},"Projection":{"ProjectionType":"ALL"}}]\'');
  await page.locator('[data-testid="tab-vec"]').click();
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="vec-table"] option').length >= 2, { timeout: 10000 });
  const modelPill = await page.locator('[data-testid="vec-model-pill"]').innerText();
  assert(modelPill.includes("Xenova/all-MiniLM-L6-v2") && modelPill.includes("384d"),
    "model pill wrong: " + modelPill);
  await page.locator('[data-testid="vec-table"]').selectOption("Pins");
  assert(await page.locator('[data-testid="vec-embed-run"]').isDisabled(),
    "text flow should be disabled for the 8-d Pins index");
  const whyEl = page.locator('[data-testid="vec-embed-why"]');
  assert(await whyEl.isVisible(), "disable reason should be visible, not tooltip-only");
  const why = await whyEl.innerText();
  assert(why.includes("8d") && why.includes("384"), "mismatch reason unhelpful: " + why);
  await page.locator('[data-testid="vec-table"]').selectOption("Quotes");
  await page.waitForFunction(() => !document.querySelector('[data-testid="vec-embed-run"]').disabled, { timeout: 10000 });
  assert(!(await whyEl.isVisible()), "disable reason should hide when dimensions match");
  console.log("  [2] dimension pinning: enabled for Quotes (384d), disabled for Pins (8d) with visible reason: " + why);

  // 3) Text query -> embed in-tab (lazy model load, visible status) ->
  //    ranked, semantically sensible results.
  await page.locator('[data-testid="vec-text"]').fill("telescopes and distant galaxies");
  await page.locator('[data-testid="vec-k"]').fill("5");
  let before = await entryCount(page);
  await page.locator('[data-testid="vec-embed-run"]').click();
  await waitNewEntry(page, before); // includes the first model load
  const status = await page.locator('[data-testid="embed-status"]').innerText();
  assert(/embedded in \d+ ms/.test(status), "embed status pill wrong: " + status);
  let entry = await newestEntry(page);
  assert(entry.text.includes("SearchVectors") && entry.text.includes("200"),
    "SearchVectors after embed failed: " + entry.text.slice(0, 300));
  // The embedded query renders as a chip (no wall of floats); expanding it
  // shows the full 384-d JSON in a fixed-height box.
  const lastQ = page.locator('[data-testid="vec-last-query"]');
  assert(await lastQ.isVisible(), "query-vector chip holder not shown after embed");
  const chipText = await lastQ.locator('[data-testid="vec-chip"]').innerText();
  assert(chipText.includes("\u00b7 384d"), "query chip not truncated with dims: " + chipText);
  await lastQ.locator('[data-testid="vec-chip-expand"]').click();
  const qvec = JSON.parse(await lastQ.locator('[data-testid="vec-full"]').innerText());
  assert(Array.isArray(qvec) && qvec.length === 384 && qvec.every(Number.isFinite),
    "expanded query chip is not a 384-d array");
  await lastQ.locator('[data-testid="vec-chip-expand"]').click();
  const srcLabel = await page.locator('[data-testid="vec-results-label"]').innerText();
  assert(srcLabel.includes("text") && srcLabel.includes("Quotes/vidx"),
    "results label does not name the text input: " + srcLabel);
  let rows = await resultRows(page);
  assert(rows.length === 5, "expected 5 ranked rows, got " + rows.length);
  const scores = rows.map((r) => Number(r[1]));
  for (let i = 1; i < scores.length; i++) assert(scores[i] >= scores[i - 1], "scores not ascending: " + scores);
  const flat0 = rows[0].join(" ");
  assert(flat0.includes("astro-01") && flat0.includes("Through a telescope"),
    "known query did not return the known top item:\n" + rows.map((r) => r.join(" | ")).join("\n"));
  console.log("  [3] 'telescopes and distant galaxies' -> top hit astro-01 (score " + rows[0][1] + "), model loaded lazily (" + status + ")");

  // 4) Determinism: the in-browser embedding of a seed sentence matches the
  //    checked-in build-time embedding within float tolerance.
  const probe = seed.items[0];
  const browserEmb = await page.evaluate(async (text) => {
    const m = await import("/embed.mjs"); // same module instance the page uses
    return await m.embedText(text);
  }, probe.text);
  assert(browserEmb.length === 384, "browser embedding has " + browserEmb.length + " dims");
  let maxDiff = 0, dot = 0;
  for (let i = 0; i < 384; i++) {
    maxDiff = Math.max(maxDiff, Math.abs(browserEmb[i] - probe.emb[i]));
    dot += browserEmb[i] * probe.emb[i];
  }
  assert(maxDiff <= 1e-3, `seed/browser embedding drift for '${probe.pk}': max abs diff ${maxDiff}`);
  assert(dot >= 0.9999, `seed/browser embedding cosine too low for '${probe.pk}': ${dot}`);
  console.log(`  [4] determinism: '${probe.pk}' build-time vs in-browser embedding: max abs diff ${maxDiff.toExponential(2)}, cosine ${dot.toFixed(6)}`);

  // 5) Add item with text (collapsible section): embed + PutItem, then find
  //    it by its own text.
  await page.locator('[data-testid="vec-sec-put"] > summary').click();
  assert(await page.locator('[data-testid="vec-sec-put"]').evaluate((d) => d.open), "put section did not open");
  await page.locator('[data-testid="vec-add-pk"]').fill("mine-01");
  await page.locator('[data-testid="vec-add-text"]').fill("The best debugging tool is a rubber duck that listens.");
  before = await entryCount(page);
  await page.locator('[data-testid="vec-add-run"]').click();
  await waitNewEntry(page, before);
  entry = await newestEntry(page);
  assert(entry.text.includes("PutItem") && entry.text.includes("200") && !entry.cls.includes("err"),
    "embed + PutItem failed: " + entry.text.slice(0, 300));
  await page.locator('[data-testid="vec-text"]').fill("The best debugging tool is a rubber duck that listens.");
  before = await entryCount(page);
  await page.locator('[data-testid="vec-embed-run"]').click();
  await waitNewEntry(page, before);
  rows = await resultRows(page);
  assert(rows[0].join(" ").includes("mine-01") && Number(rows[0][1]) < 1e-3,
    "added item should self-match first with ~0 score: " + rows[0].join(" | "));
  console.log("  [5] add-item-with-text: mine-01 inserted and self-matches (score " + rows[0][1] + ")");

  // 6) Embed -> wire vector JSON helper: a dispatch-style log entry, one-line
  //    header (chars, dims, ms), the vector JSON collapsed by default behind
  //    the standard expand control; expanded, it pastes into the CLI.
  await page.locator('[data-testid="vec-text"]').fill("rain landing on dusty ground");
  before = await entryCount(page);
  await page.locator('[data-testid="vec-embed-copy"]').click();
  await waitNewEntry(page, before);
  const helperEntry = page.locator('[data-testid="log-entry"]').first();
  entry = await newestEntry(page);
  assert(entry.text.includes("EmbedText") && /\d+-char query text/.test(entry.text) && entry.text.includes("384d") && /\d+ ms/.test(entry.text),
    "helper entry header missing chars/dims/ms: " + entry.text.slice(0, 200));
  assert(!entry.text.includes('{"N":'),
    "helper vector JSON should be collapsed by default: " + entry.text.slice(0, 200));
  await helperEntry.locator('[data-testid="log-expand"]').click();
  entry = await newestEntry(page);
  const m = entry.text.match(/(\[\{"N":".*\}\])/);
  assert(m, "expanded helper entry has no wire-format vector: " + entry.text.slice(0, 200));
  const wire = JSON.parse(m[1]);
  assert(wire.length === 384 && wire.every((e) => typeof e.N === "string" && Number.isFinite(Number(e.N))),
    "wire vector malformed");
  const cliOut = await cli(page,
    "aws dynamodb search-vectors --table-name Quotes --index-name vidx --search-vector '" + m[1] + "' --top-k 3");
  assert(cliOut.text.includes("SearchVectors") && cliOut.text.includes("SearchResults") && !cliOut.cls.includes("err"),
    "CLI search-vectors with pasted embedding failed: " + cliOut.text.slice(0, 300));
  assert(cliOut.text.includes("wx-01"), "pasted-embedding CLI search missed the petrichor quote: " + cliOut.text.slice(0, 500));
  console.log("  [6] embed helper logs collapsed; expanded JSON pastes into CLI search-vectors (top hit wx-01)");

  // 7) The model restriction is UI-only: CLI create-table still accepts
  //    arbitrary dimensions (engine behavior unchanged).
  for (const [tname, dims] of [["Tiny", 1], ["Huge", 4096]]) {
    const out = await cli(page,
      `aws dynamodb create-table --table-name ${tname} ` +
      `--attribute-definitions '[{"AttributeName":"pk","AttributeType":"S"}]' ` +
      `--key-schema '[{"AttributeName":"pk","KeyType":"HASH"}]' --billing-mode PAY_PER_REQUEST ` +
      `--vector-indexes '[{"IndexName":"vdx","Dimensions":${dims},"DistanceFunction":"EUCLIDEAN",` +
      `"VectorAttribute":{"AttributeName":"v"},"Projection":{"ProjectionType":"ALL"}}]'`);
    assert(out.text.includes("200") && !out.cls.includes("err"),
      `create-table with ${dims}-d vector index failed: ` + out.text.slice(0, 300));
  }
  console.log("  [7] CLI create-table accepts 1-d and 4096-d vector indexes (restriction is UI-only)");

  // 8) Network discipline: nothing cross-origin, ever; the lazy model load
  //    after ready was same-origin (vendor/, models/) only.
  assert(crossOrigin.length === 0, "cross-origin requests observed: " + JSON.stringify(crossOrigin));
  const badPost = postReady.filter((u) => !/\/(vendor|models)\//.test(new URL(u).pathname));
  assert(badPost.length === 0, "unexpected post-ready requests: " + JSON.stringify(badPost));
  assert(postReady.some((u) => u.includes("model_quantized.onnx")), "model was not lazy-loaded after ready");
  console.log(`  [8] zero cross-origin requests; ${postReady.length} post-ready fetches, all same-origin under /vendor/ + /models/`);

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("EMBED TEST PASSED: text -> embed -> semantic results, determinism, add-with-text section, collapsed CLI paste helper, visible dimension pinning, same-origin only");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
