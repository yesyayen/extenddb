// Copyright 2026 ExtendDB contributors
// SPDX-License-Identifier: Apache-2.0
//
// Dual-target parity, wasm side, vector sequence: vector indexes,
// SearchVectors, and the Query/Scan refusal of a vector index named as
// IndexName. Fixtures: vector-parity.requests.json + vector-parity.golden.json;
// the runner and the masking rules live in parity-lib.mjs.
//
// Run after: wasm-pack build crates/wasm --target nodejs
//   node crates/wasm/tests-node/vector-parity.mjs

import { runParity } from "./parity-lib.mjs";

runParity("vector-parity");
