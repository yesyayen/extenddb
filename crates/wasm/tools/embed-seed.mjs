// Precomputes the seed embeddings for the playground's semantic-search demo
// table (Quotes), so the page never waits on the model at load time.
//
// Determinism by construction: the texts are embedded in headless Chromium
// through web/embed.mjs, the EXACT loader, runtime, model files, and thread
// configuration the playground itself uses. Seed vectors and in-browser
// query vectors therefore come from one code path. (A node-native path via
// onnxruntime-node would be a different execution provider, and its binary
// also needs a newer glibc than this dev host has.)
//
// Output (checked in): web/data/quotes-seed.json
//
// Run:
//   cd crates/wasm/tools && npm install
//   node vendor-embed-assets.mjs     # populate web/vendor + web/models
//   node embed-seed.mjs

import http from "node:http";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { chromium } from "playwright";
import { QUOTE_TEXTS } from "./quotes-texts.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const WEB = path.resolve(__dirname, "..", "web");
const DIMENSIONS = 384;

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".onnx": "application/octet-stream",
};

const DRIVER_HTML = "<!doctype html><html><body>embed driver</body></html>";

async function startServer() {
  const server = http.createServer(async (req, res) => {
    try {
      const urlPath = decodeURIComponent(req.url.split("?")[0]);
      if (urlPath === "/" || urlPath === "/_embed-driver.html") {
        res.writeHead(200, { "content-type": MIME[".html"] }).end(DRIVER_HTML);
        return;
      }
      const filePath = path.join(WEB, path.normalize(urlPath));
      if (!filePath.startsWith(WEB)) { res.writeHead(403).end(); return; }
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
  page.on("pageerror", (e) => console.error("PAGE ERROR:", e.message));
  await page.goto(`http://127.0.0.1:${port}/_embed-driver.html`, { waitUntil: "load" });

  console.log(`  embedding ${QUOTE_TEXTS.length} texts in headless Chromium (same runtime as the playground)...`);
  const t0 = Date.now();
  const result = await page.evaluate(async (texts) => {
    const m = await import("/embed.mjs");
    const out = [];
    for (const t of texts) out.push(await m.embedText(t));
    return { model: m.EMBED_MODEL.id, dimensions: m.EMBED_MODEL.dimensions, embs: out };
  }, QUOTE_TEXTS.map((q) => q.text));
  console.log(`  done in ${((Date.now() - t0) / 1000).toFixed(1)}s`);

  if (result.dimensions !== DIMENSIONS) throw new Error(`model dims ${result.dimensions} != ${DIMENSIONS}`);
  const items = QUOTE_TEXTS.map(({ pk, topic, text }, i) => {
    const emb = result.embs[i];
    if (!Array.isArray(emb) || emb.length !== DIMENSIONS) throw new Error(`bad embedding for ${pk}`);
    return { pk, topic, text, emb };
  });

  const seed = { model: result.model, dimensions: DIMENSIONS, pooling: "mean", normalize: true, items };
  const outPath = path.join(WEB, "data", "quotes-seed.json");
  await mkdir(path.dirname(outPath), { recursive: true });
  await writeFile(outPath, JSON.stringify(seed));
  console.log(`SEED OK: ${items.length} items x ${DIMENSIONS}d -> ${outPath}`);

  await browser.close();
  await new Promise((r) => server.close(r));
}

main().catch((e) => { console.error("SEED FAILED: " + (e && e.stack || e)); process.exit(1); });
