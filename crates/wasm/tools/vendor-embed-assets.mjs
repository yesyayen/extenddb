// Vendors the in-browser embedding runtime and model into the playground's
// static assets, so everything is served same-origin with zero CDN fetches at
// runtime (required by the zero-network browser specs and the self-contained
// deployment).
//
// What it produces (both directories are gitignored, like web/pkg):
//   web/vendor/transformers/   transformers.js browser runtime + ONNX wasm
//   web/models/Xenova/all-MiniLM-L6-v2/   sentence-embedding model files
//
// Sources, both pinned:
//   runtime: @huggingface/transformers from npm (version pinned by
//            tools/package.json + package-lock.json), Apache-2.0
//   model:   Xenova/all-MiniLM-L6-v2 (all-MiniLM-L6-v2 converted to ONNX,
//            q8-quantized), pinned to a revision sha, Apache-2.0
//
// Every file is verified against a pinned sha256 so the assets that feed the
// checked-in seed embeddings (tools/embed-seed.mjs) are reproducible.
//
// Run:  cd crates/wasm/tools && npm install && node vendor-embed-assets.mjs

import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile, copyFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const WEB = path.resolve(__dirname, "..", "web");
const RUNTIME_SRC = path.resolve(__dirname, "node_modules", "@huggingface", "transformers");

export const MODEL_ID = "Xenova/all-MiniLM-L6-v2";
const MODEL_REVISION = "751bff37182d3f1213fa05d7196b954e230abad9";

// Runtime files copied out of the npm package's dist/ (plus its LICENSE).
const RUNTIME_FILES = [
  ["dist/transformers.min.js", "13746ae88695b62e431fc5ebe3beb10a080d2081406047670639ce8c10a9ba25"],
  ["dist/ort-wasm-simd-threaded.jsep.mjs", "08fb86ec433c78bfb032c5d84a68b8e8e5a8d81268fa39e24314179a5767a5b9"],
  ["dist/ort-wasm-simd-threaded.jsep.wasm", "c46655e8a94afc45338d4cb2b840475f88e5012d524509916e505079c00bfa39"],
  ["LICENSE", "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"],
];

// Model files fetched from the pinned revision. README.md carries the
// license declaration (apache-2.0) and is kept next to the weights.
const MODEL_FILES = [
  ["config.json", "7135149f7cffa1a573466c6e4d8423ed73b62fd2332c575bf738a0d033f70df7"],
  ["tokenizer.json", "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0"],
  ["tokenizer_config.json", "9261e7d79b44c8195c1cada2b453e55b00aeb81e907a6664974b4d7776172ab3"],
  ["special_tokens_map.json", "b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3"],
  ["README.md", "63ea99bf681a2e9eda4f6a537d5ed8fda95d1677111656da37e9cfd080c3af02"],
  ["onnx/model_quantized.onnx", "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1"],
];

function sha256(buf) { return createHash("sha256").update(buf).digest("hex"); }

async function verify(file, expected) {
  const got = sha256(await readFile(file));
  if (got !== expected) {
    throw new Error(`sha256 mismatch for ${file}\n  expected ${expected}\n  got      ${got}`);
  }
}

async function main() {
  const vendorDir = path.join(WEB, "vendor", "transformers");
  const modelDir = path.join(WEB, "models", ...MODEL_ID.split("/"));
  await mkdir(vendorDir, { recursive: true });
  await mkdir(path.join(modelDir, "onnx"), { recursive: true });

  for (const [rel, hash] of RUNTIME_FILES) {
    const src = path.join(RUNTIME_SRC, rel);
    const dst = path.join(vendorDir, path.basename(rel));
    await copyFile(src, dst);
    await verify(dst, hash);
    console.log("  runtime  " + path.basename(rel));
  }

  for (const [rel, hash] of MODEL_FILES) {
    const dst = path.join(modelDir, rel);
    const url = `https://huggingface.co/${MODEL_ID}/resolve/${MODEL_REVISION}/${rel}`;
    const resp = await fetch(url);
    if (!resp.ok) throw new Error(`fetch failed (${resp.status}): ${url}`);
    await writeFile(dst, Buffer.from(await resp.arrayBuffer()));
    await verify(dst, hash);
    console.log("  model    " + rel);
  }

  console.log("VENDOR OK: runtime -> web/vendor/transformers, model -> web/models/" + MODEL_ID);
}

// Allow importing MODEL_ID without side effects.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => { console.error("VENDOR FAILED: " + (e && e.stack || e)); process.exit(1); });
}
