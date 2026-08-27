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
