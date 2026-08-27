# ExtendDB browser playground

A static, dependency-free page that runs the ExtendDB engine (SQLite compiled to
WebAssembly) entirely in the browser. Every DynamoDB request executes in the tab:
no server, no network.

## Build + serve

```sh
# from the repo root: build the web-target engine wasm into web/pkg
scripts/wasm-build.sh --target web --dev --out-dir web/pkg

# build the dynein CLI wasm into web/pkg-dynein (powers the "dynein" tab)
scripts/dynein-wasm-build.sh --target web --dev --out-dir ../wasm/web/pkg-dynein

# serve this directory over http (any static server works)
cd crates/wasm/web && python3 -m http.server 8099
# open http://localhost:8099/
```

The dynein tab degrades gracefully: if `pkg-dynein` is absent the tab disables
itself and the rest of the page works.

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

The page offers four ways to drive the same in-tab database, so a write from any
one is visible to all and to the data browser:

- **CLI**: an `aws dynamodb <op> --flags` shell (parsed in JS to the wire call).
- **JS SDK**: the real `@aws-sdk/client-dynamodb` over the requestHandler shim.
- **Raw JSON**: hand-write `X-Amz-Target` + body.
- **dynein**: the real [awslabs/dynein](https://github.com/awslabs/dynein) CLI,
  vendored and compiled to a second wasm module (`pkg-dynein`). dynein's own
  DynamoDB transport is routed to this page's engine via
  `set_host_dispatch(dispatch_http)`, so `dy list`, `dy -t Music query ...`,
  `dy admin create table ...` all hit the same database. See
  `crates/dynein-wasm`.

## Scope

POC: core data plane (CreateTable / DescribeTable / ListTables / DeleteTable,
PutItem / GetItem / UpdateItem / DeleteItem with ConditionExpression, Query,
Scan). Transactions, batch, streams, backup, TTL/tags, and secondary indexes are
out of scope for the POC.

Notes:
- `python3 -m http.server` may serve `.wasm` as `application/octet-stream`; the
  wasm-bindgen loader falls back to non-streaming instantiation, so it still
  works. A server that sets `application/wasm` avoids a console warning.
- The engine performs **no authentication or authorization** on the wasm path.
  The SDK signs the request (SigV4), but the signature is discarded and never
  verified. This is a local demo, not an access-control boundary.
