// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Dual-target parity, wasm side: the browser backend must answer the request
// sequence in vector-parity.requests.json exactly as the committed golden
// (vector-parity.golden.json) says. The golden is produced and asserted by the
// native backend's `wasm_parity` test in crates/storage-sqlite, so both
// backends matching this one file is what proves they match each other,
// score bits included (the scan arithmetic is shared Rust).
//
// Masking here must stay identical to the Rust side: TableId is random and
// every *DateTime is wall-clock; ARNs are deterministic and stay unmasked.
//
// Run after: wasm-pack build crates/wasm --target nodejs
//   node crates/wasm/tests-node/vector-parity.mjs

import assert from "node:assert";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { init, dispatch_http } = require("../pkg/extenddb_wasm.js");

const here = dirname(fileURLToPath(import.meta.url));
const requests = JSON.parse(
  readFileSync(join(here, "vector-parity.requests.json"), "utf8"),
);
const golden = JSON.parse(
  readFileSync(join(here, "vector-parity.golden.json"), "utf8"),
);

const VOLATILE = [
  "CreationDateTime",
  "LastUpdateToPayPerRequestDateTime",
  "LastIncreaseDateTime",
  "LastDecreaseDateTime",
];

function mask(value) {
  if (Array.isArray(value)) {
    value.forEach(mask);
  } else if (value && typeof value === "object") {
    for (const key of Object.keys(value)) {
      if (key === "TableId") value[key] = "MASKED";
      else if (VOLATILE.includes(key)) value[key] = 0;
      else mask(value[key]);
    }
  }
  return value;
}

init();

const observed = requests.map(({ target, body }) => {
  const res = JSON.parse(
    dispatch_http(`DynamoDB_20120810.${target}`, JSON.stringify(body)),
  );
  return mask({ target, status: res.statusCode, body: JSON.parse(res.body) });
});

assert.strictEqual(
  observed.length,
  golden.length,
  "sequence length diverged from the golden",
);
observed.forEach((got, i) => {
  assert.deepStrictEqual(
    got,
    golden[i],
    `response ${i} (${got.target}) diverged from the golden`,
  );
});

console.log(`vector parity OK: ${observed.length} responses match the golden`);
