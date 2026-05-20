#!/bin/bash

cargo build --features bindgen
BINDINGS=$(ls -t ../target/debug/build/vapoursynth-sys-*/out/bindings.rs 2>/dev/null | head -1)
VSSCRIPT_BINDINGS=$(ls -t ../target/debug/build/vapoursynth-sys-*/out/vsscript_bindings.rs 2>/dev/null | head -1)

cp "$BINDINGS" ./src/bindings.rs
