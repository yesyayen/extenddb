// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Demo harness: run real dynein commands against the wasm ExtendDB engine.
//   node crates/dynein-wasm/tests-node/dy-demo.mjs
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const m = require("../pkg/extenddb_dynein_wasm.js");

const lines = [
  "admin create table dyn0 --keys pk,S sk,N",
  "list",
  "-t dyn0 put a 1 --item '{\"v\":10,\"tag\":\"x\"}'",
  "-t dyn0 put a 2 --item '{\"v\":20}'",
  "-t dyn0 put b 1 --item '{\"v\":30}'",
  "-t dyn0 get a 1",
  "-t dyn0 query a",
  "-t dyn0 scan",
  "desc -t dyn0",
];
for (const line of lines) {
  console.log("$ dy " + line);
  process.stdout.write(m.dy_exec(line));
  console.log("---");
}
