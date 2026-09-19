// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Dual-target parity, wasm side, shared runner: the browser backend must
// answer the request sequence in <name>.requests.json exactly as the committed
// golden (<name>.golden.json) says. Each golden is produced and asserted by the
// native backend's `wasm_parity` tests in crates/storage-sqlite, so both
// backends matching the same file is what proves they match each other, score
// bits and per-table capacity included (the arithmetic is shared Rust).
//
// Masking here must stay identical to the Rust side: TableId is random and
// every *DateTime is wall-clock; ARNs are deterministic and stay unmasked. The
// one order-only normalization is the same too: a ConsumedCapacity list is
// sorted by TableName, because the engine totals batch capacity per table in a
// hash map and the list order carries no meaning. Every value in it is still
// asserted.
//
// Thin entry points (vector-parity.mjs, batch-parity.mjs) call runParity with
// the fixture stem; run them after: wasm-pack build crates/wasm --target nodejs

import assert from "node:assert";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { init, dispatch_http } = require("../pkg/extenddb_wasm.js");

const here = dirname(fileURLToPath(import.meta.url));

const VOLATILE = [
  "CreationDateTime",
  "LastUpdateToPayPerRequestDateTime",
  "LastIncreaseDateTime",
  "LastDecreaseDateTime",
];

// A ConsumedCapacity entry exists only for a created table, and the engine
// restricts table names to [A-Za-z0-9_.-], so this code-unit order and the
// Rust side's byte order agree.
function compareTableName(a, b) {
  const x = a.TableName;
  const y = b.TableName;
  return x < y ? -1 : x > y ? 1 : 0;
}

export function mask(value) {
  if (Array.isArray(value)) {
    value.forEach(mask);
  } else if (value && typeof value === "object") {
    for (const key of Object.keys(value)) {
      if (key === "TableId") value[key] = "MASKED";
      else if (VOLATILE.includes(key)) value[key] = 0;
      else {
        if (key === "ConsumedCapacity" && Array.isArray(value[key])) {
          value[key].sort(compareTableName);
        }
        mask(value[key]);
      }
    }
  }
  return value;
}

// Run the named sequence over a fresh engine and assert its golden.
export function runParity(name) {
  const requests = JSON.parse(
    readFileSync(join(here, `${name}.requests.json`), "utf8"),
  );
  const golden = JSON.parse(
    readFileSync(join(here, `${name}.golden.json`), "utf8"),
  );

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
    `${name}: sequence length diverged from the golden`,
  );
  observed.forEach((got, i) => {
    assert.deepStrictEqual(
      got,
      golden[i],
      `${name}: response ${i} (${got.target}) diverged from the golden`,
    );
  });

  console.log(`${name} OK: ${observed.length} responses match the golden`);
}
