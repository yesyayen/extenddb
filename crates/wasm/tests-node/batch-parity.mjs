// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Dual-target parity, wasm side, batch sequence: BatchWriteItem and
// BatchGetItem fan-out across two tables, their validation errors, and the
// follow-up reads that prove the writes landed. Fixtures:
// batch-parity.requests.json + batch-parity.golden.json; the runner and the
// masking rules live in parity-lib.mjs.
//
// Run after: wasm-pack build crates/wasm --target nodejs
//   node crates/wasm/tests-node/batch-parity.mjs

import { runParity } from "./parity-lib.mjs";

runParity("batch-parity");
