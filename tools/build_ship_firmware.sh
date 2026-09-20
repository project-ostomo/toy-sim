#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
firmware_target="${CARGO_TARGET_DIR:-target}"
cargo build -p osg-example-controller --release --target wasm32-unknown-unknown --lib --offline
cp "$firmware_target"/wasm32-unknown-unknown/release/osg_example_controller.wasm crates/osg-ships/data/example-controller.wasm
cargo build -p osg-example-controller --release --target wasm32-unknown-unknown --no-default-features --example custom_screen --offline
cargo build -p osg-example-controller --release --target wasm32-unknown-unknown --no-default-features --example chatter --offline
cargo build -p osg-ship-api --release --target wasm32-unknown-unknown --example embedded --offline
clang --target=wasm32 -O2 -nostdlib -fno-builtin \
    -I crates/osg-ship-api/include \
    -Wl,--no-entry -Wl,--export-memory -Wl,-z,stack-size=65536 -Wl,--max-memory=8388608 \
    crates/osg-ship-wasm/tests/fixtures/controller.c \
    -o crates/osg-ship-wasm/tests/fixtures/controller.wasm
cp "$firmware_target"/wasm32-unknown-unknown/release/examples/custom_screen.wasm crates/osg-ship-wasm/tests/fixtures/custom-screen.wasm
cp "$firmware_target"/wasm32-unknown-unknown/release/examples/chatter.wasm crates/osg-ships/data/chatter-controller.wasm
cp "$firmware_target"/wasm32-unknown-unknown/release/examples/embedded.wasm crates/osg-ship-wasm/tests/fixtures/embedded.wasm
