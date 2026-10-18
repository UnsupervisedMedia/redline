#!/usr/bin/env bash
# Regenerate THIRD_PARTY_LICENSES.md from Cargo.lock.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --locked | python3 -I -c '
import json, sys
m = json.load(sys.stdin)
rows = sorted({(p["name"], p["version"], p.get("license") or p.get("license_file") or "UNKNOWN", p.get("repository") or "")
               for p in m["packages"] if p["name"] != "redline"})
out = ["# Third-party licenses\n",
       "Redline is licensed under Apache-2.0 (see `LICENSE`). It links the Rust crates below, each under its own license; "
       "the full text of each license ships inside the crate source package on crates.io. Generated from `Cargo.lock` by "
       "`scripts/gen-third-party.sh`. Bundled assets and downloaded models are covered in `docs/LICENSING.md`.\n",
       "| Crate | Version | License | Source |", "|---|---|---|---|"]
out += [f"| {n} | {v} | {l} | {r} |" for n, v, l, r in rows]
open("THIRD_PARTY_LICENSES.md", "w").write("\n".join(out) + "\n")
'
