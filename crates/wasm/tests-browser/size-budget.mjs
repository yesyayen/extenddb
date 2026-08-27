// Size budget for the shippable browser wasm (U5 ship polish).
//
// Measures the gzipped size of the optimized (wasm-opt) release build and
// asserts it is under budget. wasm-opt shrinks the dev artifact from ~10 MB to
// ~2.7 MB raw (~1.0 MB gzipped), well under DuckDB-WASM's ~30 MB; the budget
// leaves headroom for growth.
//
// Build the release artifact first (a couple of minutes in the container):
//   scripts/wasm-build.sh --target web --out-dir web/pkg-release
// Then:
//   cd crates/wasm/tests-browser && node size-budget.mjs
// Optionally point at a different artifact:
//   node size-budget.mjs ../web/pkg/extenddb_wasm_bg.wasm 6

import { readFile, stat } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const target = process.argv[2]
  ? path.resolve(process.cwd(), process.argv[2])
  : path.resolve(__dirname, "..", "web", "pkg-release", "extenddb_wasm_bg.wasm");
const budgetMB = process.argv[3] ? Number(process.argv[3]) : 2.0; // gzipped
const budget = budgetMB * 1024 * 1024;

function mb(n) { return (n / 1024 / 1024).toFixed(2) + " MB"; }

async function main() {
  try { await stat(target); }
  catch {
    console.error("SIZE BUDGET: artifact not found: " + target +
      "\n  build the release artifact: scripts/wasm-build.sh --target web --out-dir web/pkg-release");
    process.exit(1);
  }
  const buf = await readFile(target);
  const gz = gzipSync(buf, { level: 9 }).length;
  console.log(`  artifact:  ${target}`);
  console.log(`  raw:       ${mb(buf.length)} (${buf.length} bytes)`);
  console.log(`  gzipped:   ${mb(gz)} (${gz} bytes)`);
  console.log(`  budget:    ${mb(budget)} gzipped`);
  if (gz > budget) {
    console.error(`SIZE BUDGET FAILED: gzipped ${mb(gz)} exceeds ${mb(budget)}`);
    process.exit(1);
  }
  console.log(`SIZE BUDGET PASSED: gzipped ${mb(gz)} within ${mb(budget)}`);
}

main().catch((e) => { console.error("SIZE BUDGET FAILED: " + e); process.exit(1); });
