// Text-embedding support for the ExtendDB browser playground.
//
// One place pins the model, its dimensions, and the loader configuration.
// Three consumers share it so seed vectors, in-page query vectors, and test
// vectors all come from the same model and the same runtime:
//   - index.html (the Vectors tab text flow)
//   - tools/embed-seed.mjs (build-time seed embeddings, via headless Chromium)
//   - tests-browser/embed.spec.mjs (the determinism check)
//
// Everything loads same-origin from the playground's own static assets
// (web/vendor/transformers + web/models), populated by
// tools/vendor-embed-assets.mjs. There is no CDN fetch at runtime.

// The single pinned model. The Vectors tab enables the text flow only for
// vector indexes whose dimension matches EMBED_MODEL.dimensions, so seed and
// query vectors can never come from different models.
export const EMBED_MODEL = {
  id: "Xenova/all-MiniLM-L6-v2", // all-MiniLM-L6-v2 converted to ONNX, q8
  dimensions: 384,
  license: "Apache-2.0",
};

let embedderPromise = null;

// Lazily creates (once) the feature-extraction pipeline from the vendored
// runtime and model. progress_callback receives transformers.js progress
// events ({ status, file, progress, ... }) during the first load only.
export function getEmbedder(progress_callback) {
  if (!embedderPromise) {
    embedderPromise = (async () => {
      const { pipeline, env } = await import("./vendor/transformers/transformers.min.js");
      // Same-origin only: local model dir, local ONNX wasm, no remote fallback.
      env.allowRemoteModels = false;
      env.allowLocalModels = true;
      env.localModelPath = new URL("./models/", import.meta.url).href;
      env.backends.onnx.wasm.wasmPaths = new URL("./vendor/transformers/", import.meta.url).href;
      // Single-threaded: deterministic, and needs no cross-origin isolation.
      env.backends.onnx.wasm.numThreads = 1;
      return await pipeline("feature-extraction", EMBED_MODEL.id, { dtype: "q8", progress_callback });
    })();
    embedderPromise.catch(() => { embedderPromise = null; }); // allow retry after a failed load
  }
  return embedderPromise;
}

// Embeds one text into a unit-length EMBED_MODEL.dimensions vector (plain
// number[]), mean-pooled and L2-normalized, pairing with a COSINE index.
export async function embedText(text, progress_callback) {
  const embed = await getEmbedder(progress_callback);
  const out = await embed(String(text), { pooling: "mean", normalize: true });
  const v = Array.from(out.data);
  if (v.length !== EMBED_MODEL.dimensions) {
    throw new Error(`model returned ${v.length} dims, expected ${EMBED_MODEL.dimensions}`);
  }
  return v;
}
