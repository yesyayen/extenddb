// Headless-browser test for the ExtendDB browser playground (U5 shareable link).
//
// Verifies the "Share link" flow: the current CLI command is encoded into the
// URL hash, and opening a #cmd=... link prefills the CLI and auto-runs it, so a
// shared link reproduces the demo. Zero network throughout.
//
// Run:  cd crates/wasm/tests-browser && node share.spec.mjs

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

function fail(msg) { console.error("SHARE TEST FAILED: " + msg); process.exit(1); }
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
      const urlPath = decodeURIComponent(req.url.split("?")[0].split("#")[0]);
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
  const base = `http://127.0.0.1:${port}/index.html`;
  const browser = await chromium.launch();

  // Part A: create a shareable link from the current command.
  {
    const page = await browser.newPage();
    page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));
    await page.goto(base, { waitUntil: "load" });
    await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });

    const cmd = "aws dynamodb list-tables";
    await page.fill('[data-testid="cli"]', cmd);
    await page.locator('[data-testid="cli-share"]').click();

    await page.waitForFunction(() => location.hash.startsWith("#cmd="), { timeout: 5000 });
    const hash = await page.evaluate(() => location.hash);
    const decoded = decodeURIComponent(hash.replace(/^#cmd=/, ""));
    assert(decoded === cmd, `hash did not encode the command: got "${decoded}"`);
    const log = await page.locator('[data-testid="log"]').innerText();
    assert(/Shareable link/.test(log), "no shareable-link message logged");
    console.log("  [A] Share link encodes the current command into #cmd=... and reports the URL");
    await page.close();
  }

  // Part B: opening a #cmd=... link prefills the CLI and auto-runs it.
  {
    const page = await browser.newPage();
    let readyReached = false;
    const postReadyRequests = [];
    page.on("request", (req) => { if (readyReached) postReadyRequests.push(req.url()); });
    page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));

    const cmd =
      'aws dynamodb query --table-name Music --key-condition-expression "Artist = :a" --expression-attribute-values \'{":a":{"S":"Radiohead"}}\'';
    await page.goto(base + "#cmd=" + encodeURIComponent(cmd), { waitUntil: "load" });
    await page.waitForFunction(() => document.body.getAttribute("data-ready") === "true", { timeout: 30000 });
    readyReached = true;

    // The CLI input is prefilled from the link.
    await page.waitForFunction(
      (c) => document.querySelector('[data-testid="cli"]').value === c,
      cmd,
      { timeout: 10000 }
    );
    // And it auto-ran: the query result is in the log.
    await page.waitForFunction(
      () => {
        const t = document.querySelector('[data-testid="log"]').textContent;
        return t.includes("Paranoid Android") && t.includes("Karma Police");
      },
      { timeout: 10000 }
    );
    assert(postReadyRequests.length === 0,
      "network after ready on a shared link: " + JSON.stringify(postReadyRequests));
    console.log("  [B] opening #cmd=... prefills the CLI and auto-runs it (zero network)");
    await page.close();
  }

  await browser.close();
  await new Promise((r) => server.close(r));
  console.log("SHARE TEST PASSED: shareable link round-trips (encode current command, restore + auto-run on open)");
}

main().catch((e) => fail(String(e && e.stack ? e.stack : e)));
