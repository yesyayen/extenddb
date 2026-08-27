// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// AWS SDK v3 requestHandler that routes DynamoDB requests to the in-browser
// ExtendDB wasm engine with ZERO network. Drop it into any DynamoDBClient:
//
//   import * as wasm from "extenddb-wasm";
//   wasm.init();
//   const client = new DynamoDBClient({
//     region: "us-east-1",
//     credentials: { accessKeyId: "x", secretAccessKey: "y" },
//     requestHandler: createExtenddbRequestHandler(wasm),
//   });
//   await client.send(new PutItemCommand({ ... }));   // runs in the tab
//
// The SDK still builds/serializes the request and signs it; we ignore the
// signature and dispatch the JSON body against the wasm engine, then hand back
// a synthesized HttpResponse so the SDK's normal deserializer + error mapping
// runs unchanged (non-2xx + __type -> the right exception class).
//
// SECURITY: the wasm engine performs NO authentication or authorization. The
// SDK signs the request (SigV4), but the signature is discarded and never
// verified. This is a local, single-tab demo, not an access-control boundary.

// The SDK's deserializer only reads response.statusCode / .headers / .body, so
// a structural object works and keeps this shim dependency-free (no smithy import).

/// Browser/worker response body: fetch-handler streamCollector reads a Blob.
export function browserBody(str) {
  return new Blob([new TextEncoder().encode(str)]);
}

/// Node response body: node-http-handler streamCollector reads a Readable.
let _nodeStream;
export async function nodeBody(str) {
  _nodeStream ??= await import("node:stream");
  return _nodeStream.Readable.from(Buffer.from(str, "utf8"));
}

function requestId() {
  if (typeof crypto !== "undefined" && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return Math.random().toString(16).slice(2).padEnd(16, "0");
}

/**
 * @param wasm the initialized extenddb-wasm module (exposing dispatch_http).
 * @param opts.makeBody how to wrap the response body string; defaults to a
 *   browser Blob. Pass `nodeBody` when running under Node.
 */
export function createExtenddbRequestHandler(wasm, { makeBody = browserBody } = {}) {
  return {
    metadata: { handlerProtocol: "http/1.1" },
    async handle(request) {
      const headers = request.headers || {};
      const target = headers["x-amz-target"] ?? headers["X-Amz-Target"] ?? "";
      const bodyStr =
        typeof request.body === "string"
          ? request.body
          : new TextDecoder().decode(request.body ?? new Uint8Array());

      const { statusCode, body } = JSON.parse(wasm.dispatch_http(target, bodyStr));

      return {
        // Structural response (no @smithy/protocol-http dependency). @smithy
        // duck-types HttpResponse.isInstance() on statusCode/headers, so this is
        // accepted; if a future SDK major changes that contract, swap to
        // `new HttpResponse({ ... })` from @smithy/protocol-http.
        response: {
          statusCode,
          headers: {
            "content-type": "application/x-amz-json-1.0",
            "x-amzn-requestid": requestId(),
          },
          body: await makeBody(body),
        },
      };
    },
  };
}
