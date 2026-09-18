#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
cargo build -p toy-sim-example-controller --release --target wasm32-unknown-unknown --lib --offline
cp target/wasm32-unknown-unknown/release/toy_sim_example_controller.wasm crates/toy-sim-ships/data/example-controller.wasm
cargo build -p toy-sim-example-controller --release --target wasm32-unknown-unknown --no-default-features --example custom_screen --offline
cargo build -p toy-sim-ship-api --release --target wasm32-unknown-unknown --example embedded --offline
clang --target=wasm32 -O2 -nostdlib -fno-builtin \
    -I crates/toy-sim-ship-api/include \
    -Wl,--no-entry -Wl,--export-memory -Wl,-z,stack-size=65536 -Wl,--max-memory=1048576 \
    crates/toy-sim-ship-wasm/tests/fixtures/controller.c \
    -o crates/toy-sim-ship-wasm/tests/fixtures/controller.wasm
cp target/wasm32-unknown-unknown/release/examples/custom_screen.wasm crates/toy-sim-ship-wasm/tests/fixtures/custom-screen.wasm
cp target/wasm32-unknown-unknown/release/examples/embedded.wasm crates/toy-sim-ship-wasm/tests/fixtures/embedded.wasm
