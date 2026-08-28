# ExtendDB browser playground

A static, dependency-free page that runs the ExtendDB engine (SQLite compiled to
WebAssembly) entirely in the browser. Every DynamoDB request executes in the tab:
no server, no network.

## Build + serve

```sh
# from the repo root: build the web-target engine wasm into web/pkg
scripts/wasm-build.sh --target web --dev --out-dir web/pkg

# vendor the text-embedding runtime + model into web/vendor + web/models
# (powers the Vector Workbench's semantic text search; optional, degrades gracefully)
cd crates/wasm/tools && npm install && node vendor-embed-assets.mjs && cd -

# serve this directory over http (any static server works)
cd crates/wasm/web && python3 -m http.server 8099
# open http://localhost:8099/
```

The embedding assets degrade gracefully: without `web/vendor` + `web/models`
the Vector Workbench's text flow reports the model as unavailable and everything
else works.

The seeded `Quotes` table's 384-d sentence embeddings live in
`web/data/quotes-seed.json` (checked in), precomputed by
`tools/embed-seed.mjs` with the same model the browser loads for query text
(`web/embed.mjs` pins it). The model lazy-loads same-origin on first use;
there is no CDN fetch at runtime.

Load a sample, edit the JSON, and hit Run. `Reset engine` gives a fresh
in-memory database.

## Using the real AWS SDK v3 (zero network)

`sdk-request-handler.mjs` is a drop-in `requestHandler` so a real
`@aws-sdk/client-dynamodb` client talks to this engine with no network:

```js
import initWasm, * as wasm from "./pkg/extenddb_wasm.js";
import { createExtenddbRequestHandler } from "./sdk-request-handler.mjs";
await initWasm();
wasm.init();
const client = new DynamoDBClient({
  region: "us-east-1",
  credentials: { accessKeyId: "x", secretAccessKey: "y" },
  requestHandler: createExtenddbRequestHandler(wasm),
});
await client.send(new PutItemCommand({ /* ... */ })); // runs in the tab
```

See `../tests-node/sdk-integration.mjs` for the Node integration test.

## Interfaces (one shared engine)

The tab row holds two groups (see `UI-GRAMMAR.md` for the full UI rules):
**Clients** speak the wire protocol the way a customer would; **Tools** are
form-driven surfaces that compose real wire calls. All of them drive the same
in-tab database, so a write from any one is visible to all and to the data
browser:

- **CLI** (client): an `aws dynamodb <op> --flags` shell (parsed in JS to the wire call).
- **JS SDK** (client): the real `@aws-sdk/client-dynamodb` over the requestHandler shim.
- **Raw JSON** (client): hand-write `X-Amz-Target` + body.
- **Vector Workbench** (tool): collapsible form-driven sections, today one
  `Vectors` subsection: a shared table/index context bar, create a table with
  a vector index (index name, dimensions, distance metric), put/update an item
  from plain text (embedded in-tab) with optional flat attributes, and
  `SearchVectors` with query text (primary)
  or a raw JSON vector (advanced). Results render as a ranked table with
  scores. The seeded `Quotes` table carries a COSINE index (`vidx`) over real
  384-d sentence embeddings, so semantic search works out of the box. The
  data browser marks vector-indexed tables with a pill and renders vector
  attributes as truncating chips with expand/copy.

The seeded `Books` table (30 rows, `Author` + `Title` composite key) is the
general-purpose non-vector demo table for the client tabs.

## Scope

POC: core data plane (CreateTable / DescribeTable / ListTables / DeleteTable,
PutItem / GetItem / UpdateItem / DeleteItem with ConditionExpression, Query,
Scan) plus vector indexes declared at CreateTable and SearchVectors (exact
nearest-neighbor scan, COSINE / EUCLIDEAN / DOT_PRODUCT). Transactions, batch,
streams, backup, TTL/tags, and non-vector secondary indexes are out of scope
for the POC.

Notes:
- `python3 -m http.server` may serve `.wasm` as `application/octet-stream`; the
  wasm-bindgen loader falls back to non-streaming instantiation, so it still
  works. A server that sets `application/wasm` avoids a console warning.
- The engine performs **no authentication or authorization** on the wasm path.
  The SDK signs the request (SigV4), but the signature is discarded and never
  verified. This is a local demo, not an access-control boundary.
