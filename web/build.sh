#!/bin/sh
# builds the wasm module into web/www/pkg
set -eu
cd "$(dirname "$0")/.."
cargo build --locked -p yoink-web --target wasm32-unknown-unknown --release -q
wasm-bindgen target/wasm32-unknown-unknown/release/yoink_web.wasm --target web --out-dir web/www/pkg --no-typescript
