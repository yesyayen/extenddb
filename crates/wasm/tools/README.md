# wasm engine dev tools

Dev/test harnesses for the ExtendDB wasm engine. None of this ships in the
browser artifact; the browser demo stays zero-network.

## http-bridge.mjs

Loopback HTTP bridge exposing the nodejs wasm pkg (`crates/wasm/pkg`, built by
`scripts/wasm-build.sh --target nodejs`) as a local DynamoDB JSON endpoint.
Lets native clients (AWS CLI, curl, boto3) drive the exact same wasm
engine the browser runs.

```bash
node crates/wasm/tools/http-bridge.mjs 8123
curl -s -X POST http://127.0.0.1:8123/ \
  -H 'X-Amz-Target: DynamoDB_20120810.ListTables' -d '{}'
```

Binds `127.0.0.1` only. No auth: SigV4 signatures are accepted and ignored
(same posture as `web/sdk-request-handler.mjs` and DynamoDB Local). One
in-memory engine per process; state vanishes on exit.

## vendor-embed-assets.mjs

Vendors the in-browser text-embedding runtime and model into the playground's
static assets (`web/vendor/transformers/`, `web/models/`; both gitignored,
like `web/pkg`), so the semantic-search demo serves everything same-origin
with zero CDN fetches at runtime. Runtime is `@huggingface/transformers`
(pinned by `package.json` + lockfile, Apache-2.0); model is
Xenova/all-MiniLM-L6-v2 q8 ONNX (pinned to a revision sha, Apache-2.0).
Every file is verified against a pinned sha256.

```bash
cd crates/wasm/tools && npm install
node vendor-embed-assets.mjs
```

## embed-seed.mjs

Precomputes the 384-d seed embeddings for the playground's `Quotes` table
(texts in `quotes-texts.mjs`, all original) and writes the checked-in
`web/data/quotes-seed.json`. Embeds in headless Chromium through
`web/embed.mjs`, the exact loader + runtime + model the page itself uses, so
seed vectors and in-browser query vectors are bit-identical for the same
text. Re-run after changing the texts, the model pin, or the runtime pin:

```bash
cd crates/wasm/tools && node vendor-embed-assets.mjs && node embed-seed.mjs
```
