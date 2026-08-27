# ExtendDB browser playground: headless-browser tests

Real end-to-end tests for `crates/wasm/web/index.html`, driven in a headless
Chromium via [Playwright](https://playwright.dev). This closes the last-mile
gap that the Node smoke and SDK-integration tests cannot cover: actual page
load, wasm boot in a browser, DOM interaction, and browser-level network
observation.

The page uses **mode tabs** (CLI / JS SDK / Raw JSON / Vectors / dynein): exactly one console is
visible at a time, so specs click the relevant `tab-*` before driving a panel.
The **log** is a scrollable column of boxed, millisecond-timestamped entries on
the left (each entry has `data-testid="log-entry"`; error entries carry `.err`;
long bodies clamp to ~5 lines with an inline expand toggle, so there is only one
scroll). Readiness is signalled by `document.body[data-ready="true"]` (there is
no status pill). The theme defaults to **light** with a dark toggle. The CLI is a
textarea with grouped, bash-formatted sample commands.

- `ui.spec.mjs` (U1): the demo shell (boot, pre-seed, run op, error marker, reset, zero network).
- `cli.spec.mjs` (U2): the AWS CLI-style shell (`aws dynamodb <op> --flags`).
- `sdk.spec.mjs` (U3): the in-page real `@aws-sdk/client-dynamodb` console.
- `browser.spec.mjs` (U4): the data browser (table selector + live item grid).
- `share.spec.mjs` (U5): the shareable link (encode command in URL, restore + auto-run).
- `vector.spec.mjs`: the Vectors tab (create a vector-indexed table from the
  form, insert via CLI, SearchVectors with a picked item or a typed vector,
  ranked results with scores, data-browser pill + truncated vector previews,
  raw JSON + CLI `search-vectors` parity, zero network).
- `size-budget.mjs` (U5): gzipped size budget for the optimized release wasm.

## What `ui.spec.mjs` asserts

1. The WebAssembly engine boots (`body[data-ready="true"]`).
2. The page is **pre-seeded** (never a blank prompt): the funny boot note reports
   `Seeded 30 tracks`.
3. Running the pre-loaded `Query` in the Raw JSON console renders a boxed `200`
   entry with the seeded Radiohead items.
4. A non-2xx response (`DescribeTable` after `DeleteTable`) renders as an
   **error entry** (`.err` box + `ResourceNotFoundException`).
5. `Reset engine` re-seeds a fresh in-memory database (`re-seeded Music with 30 items`).
6. After ready, the browser observes **zero** network requests while running
   operations (Playwright's own `request` events).
7. Exactly one `.wasm` fetch happened, at load (affirmative load-once proof).

## Prerequisites

- A web build present at `crates/wasm/web/pkg`:
  ```
  scripts/wasm-build.sh --target web --dev --out-dir web/pkg
  ```
- The Playwright Chromium browser (one-time, downloads to `~/.cache/ms-playwright`):
  ```
  npx playwright install chromium-headless-shell
  ```
  On hosts whose OS is not officially supported (e.g. Amazon Linux 2),
  Playwright downloads a compatible fallback build automatically; the
  headless shell has been verified to launch and run there.
- For `sdk.spec.mjs` only: the in-page SDK bundle (`web/pkg-sdk`):
  ```
  cd crates/wasm/web-sdk && npm install && npm run build
  ```

## Run

```
cd crates/wasm/tests-browser
npm install
node ui.spec.mjs      # U1 demo shell
node cli.spec.mjs     # U2 CLI-style shell
node sdk.spec.mjs     # U3 in-page real AWS SDK v3 (needs web/pkg-sdk built)
node browser.spec.mjs # U4 data browser (table selector + live item grid)
node share.spec.mjs   # U5 shareable link
# or all:  npm test
```

## What `share.spec.mjs` asserts (U5)

1. Clicking "Share link" encodes the current CLI command into the URL hash
   (`#cmd=...`) and reports the shareable URL.
2. Opening a `#cmd=...` link prefills the CLI input and auto-runs it, so the
   shared link reproduces the demo, with zero network.

## Size budget (U5)

`size-budget.mjs` asserts the gzipped size of the optimized (wasm-opt) release
build is under budget. Build it first, then check:

```
scripts/wasm-build.sh --target web --out-dir web/pkg-release   # ~2 min (wasm-opt)
cd crates/wasm/tests-browser && npm run size
```

Measured: ~2.7 MB raw, ~1.0 MB gzipped (the dev build is ~10 MB / ~2.6 MB
gzipped; `wasm-opt` does the shrinking). Budget: 2 MB gzipped.

## What `browser.spec.mjs` asserts (U4)

1. On boot the grid renders the pre-seeded `Music` table (30 rows, key columns marked).
2. A `PutItem` (via the Raw console) shows up as a new grid row with no manual
   refresh (30 -> 31, count badge updates).
3. Zero network after ready: the data browser scans in-tab.

## What `sdk.spec.mjs` asserts (U3)

1. The bundled real `@aws-sdk/client-dynamodb` (`window.ExtendDBSDK`) loads and
   the in-page client is wired to the wasm engine.
2. `client.send(new QueryCommand(...))` round-trips through the engine and
   returns the seeded items.
3. A failed `ConditionExpression` surfaces the SDK's typed
   `ConditionalCheckFailedException`.
4. An SDK `PutItem` is visible to a subsequent SDK `GetItem`.
5. A 60-item bulk insert via the SDK console caps the data-browser grid at 50
   rendered rows (no unbounded DOM).
6. The SDK signs every request, yet zero network is observed after the engine
   is ready: it all runs in-tab.

## What `cli.spec.mjs` asserts (U2)

1. `aws dynamodb list-tables` maps to `ListTables`, returns the seeded `Music`
   table, and the equivalent wire call is reflected into the raw console.
2. `query` with a quoted key-condition + single-quoted JSON `--expression-attribute-values`
   parses and returns the seeded Radiohead items.
3. `put-item` then `get-item` round-trips a CLI-written item through the engine.
4. `scan --limit 2` coerces `--limit` to a numeric `Limit` and caps `Count`.
5. An invalid JSON flag surfaces a parse-error entry without dispatching.
6. The `--flag=value` form (botocore-style) is accepted.
7. A non-numeric `--limit` is rejected with a parse error (not silently null).
8. **Control plane via the CLI**: `create-table` then `list-tables` shows the new table.
9. Zero network after the engine is ready: the CLI shell dispatches in-tab.

Exit code 0 and `UI TEST PASSED` on success; non-zero with `UI TEST FAILED` and
the failing assertion otherwise. The test starts its own local static file
server (correct `application/wasm` MIME) on an ephemeral port, so nothing needs
to be running beforehand.
