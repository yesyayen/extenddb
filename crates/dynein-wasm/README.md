# extenddb-dynein-wasm

The real [awslabs/dynein](https://github.com/awslabs/dynein) command layer
(vendored, Apache-2.0) compiled to WebAssembly, driving `extenddb_engine::dispatch`
in-process. No network, no bridge, no JS bounce: dynein's DynamoDB client
transport is swapped for a custom aws-smithy `HttpConnector` that calls the
ExtendDB engine directly, all in one wasm module.

This is the in-browser answer to "a real, official AWS DynamoDB CLI over
ExtendDB": dynein is Rust, so its DynamoDB client transport is a swappable
trait and the whole stack ports cleanly to wasm.

## What runs

`dy_exec(line)` parses a dynein command line with dynein's real clap parser and
executes it, returning captured stdout. Working commands (data plane + table
admin):

- `admin create table <name> --keys pk,S [sk,N]`, `list`, `desc`, `admin delete table`
- `put <pk> [sk] --item '{...}'`, `get`, `del`, `upd`
- `query <pk> [--sort-key ...]`, `scan`

Not vendored (fs/network-heavy dynein modules): `bootstrap`, `export`/`import`,
`backup`/`restore`, `bwrite`, all-regions listing. They report "not supported
in the browser demo".

## How it works

- `engine_bridge.rs`: `EngineConnector` (an `aws_smithy_runtime_api` `HttpConnector`)
  pulls `X-Amz-Target` + JSON body off each request and calls
  `extenddb_engine::dispatch` over an in-memory SQLite-wasm backend, then
  synthesizes the `HttpResponse`. `wasm_sdk_config()` hand-builds an `SdkConfig`
  (no aws-config, no network) with retries/timeouts/stalled-stream-protection
  disabled, `IdentityCache::no_cache`, a `StaticTimeSource` (wasm has no
  `SystemTime::now`), and the connector as `http_client`.
- `lib.rs`: shadows `println!`/`print!` crate-wide to capture dynein's stdout
  into a buffer that `dy_exec` returns. `app.rs::build_sdk_config` returns the
  wasm config on `wasm32`; fs/home/aws-config sites are gated off wasm.

State (an in-memory engine) is a thread-local that persists across `dy_exec`
calls within one wasm instance, and vanishes when the instance is dropped.

## Shared-engine mode (the browser demo)

By default the connector calls the engine linked into this module. For the
browser playground we want every interface (CLI / JS SDK / Raw / dynein) to
share ONE database, so dynein's transport is instead routed to the demo's main
engine module. Register a host dispatch once, then dynein stops using its own
engine and every request goes to the shared one:

```js
import initDynein, { dy_exec, set_host_dispatch } from "./pkg-dynein/extenddb_dynein_wasm.js";
await initDynein();
// dispatch_http comes from the main engine module; same {statusCode, body} shape.
set_host_dispatch((target, body) => dispatch_http(target, body));
dy_exec("-t Music query Radiohead"); // runs against the shared engine
```

`clear_host_dispatch()` reverts to the in-process engine. This is a JS-mediated
hop between two wasm instances, still zero network.

## Build and run

```bash
scripts/dynein-wasm-build.sh                 # wasm-pack build in the modern-clang container
node crates/dynein-wasm/tests-node/dy-demo.mjs

# web-target build for the browser playground's "dynein" tab:
scripts/dynein-wasm-build.sh --target web --dev --out-dir ../wasm/web/pkg-dynein
```

Standalone crate (own `[workspace]`, excluded from the ExtendDB workspace) so
the AWS SDK dep tree never enters the shipped ExtendDB build.
