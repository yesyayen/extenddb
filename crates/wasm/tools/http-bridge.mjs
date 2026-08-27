#!/usr/bin/env node
// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Loopback HTTP bridge: exposes the ExtendDB wasm engine (nodejs pkg) as a
// local DynamoDB JSON endpoint so native clients (AWS CLI, curl, boto3) can
// drive it. This is a dev/test harness, NOT the browser artifact: the browser
// demo stays zero-network; this deliberately opens a loopback socket, the
// same trust model as DynamoDB Local.
//
//   node crates/wasm/tools/http-bridge.mjs [port]     (default 8100)
//
// SECURITY: binds 127.0.0.1 only. No authentication; SigV4 signatures are
// accepted and ignored (identical posture to web/sdk-request-handler.mjs).
// State is one in-memory engine per process; it vanishes on exit.

import { createRequire } from "node:module";
import http from "node:http";
import crypto from "node:crypto";

const require = createRequire(import.meta.url);
// nodejs-target wasm-pack output (CommonJS), built by scripts/wasm-build.sh --target nodejs
const wasm = require("../pkg/extenddb_wasm.js");

wasm.init();

const PORT = Number(process.argv[2] ?? process.env.EXTENDDB_BRIDGE_PORT ?? 8100);
const HOST = "127.0.0.1"; // loopback only, by design

const server = http.createServer((req, res) => {
  if (req.method !== "POST") {
    res.writeHead(405, { "content-type": "application/json" });
    res.end('{"message":"POST only"}');
    return;
  }
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const target = String(req.headers["x-amz-target"] ?? "");
    const body = Buffer.concat(chunks).toString("utf8");
    let statusCode;
    let respBody;
    try {
      ({ statusCode, body: respBody } = JSON.parse(wasm.dispatch_http(target, body)));
    } catch (e) {
      statusCode = 500;
      respBody = JSON.stringify({
        __type: "com.amazonaws.dynamodb.v20120810#InternalServerError",
        message: `bridge: dispatch failed: ${e}`,
      });
    }
    const op = target.split(".").pop() || "(no target)";
    console.error(`${new Date().toISOString()} ${op} -> ${statusCode}`);
    res.writeHead(statusCode, {
      "content-type": "application/x-amz-json-1.0",
      "x-amzn-requestid": crypto.randomUUID(),
    });
    res.end(respBody);
  });
});

server.listen(PORT, HOST, () => {
  console.error(`ExtendDB wasm bridge listening on http://${HOST}:${PORT}`);
});
