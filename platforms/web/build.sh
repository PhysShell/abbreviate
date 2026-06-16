#!/usr/bin/env bash
# Builds the web demo: compiles the wasm module and stages the lexicon + LM
# next to the page. Run from the repo root or this directory.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

# 1. wasm module → platforms/web/pkg
#    (wasm-opt is fetched from GitHub at build time; disabled in the crate
#     metadata, so an unoptimized but working module is produced offline.)
wasm-pack build "$root/crates/abbrev-wasm" --target web --release \
  --out-dir "$here/pkg"

# 2. data artifacts → platforms/web/assets (served by relative fetch)
mkdir -p "$here/assets"
cp "$root/data/lexicons/ru-50k.tsv" "$here/assets/lexicon.tsv"
cp "$root/data/lm/ru-lm.tsv" "$here/assets/lm.tsv"
cp "$root/data/shortcuts/ru.tsv" "$here/assets/shortcuts.tsv"
cp "$root/data/lexicons/ru-hold-groups.tsv" "$here/assets/hold-groups.tsv"

echo "built. serve with:  python3 -m http.server -d $here 8000"
echo "then open:          http://localhost:8000/"
