#!/usr/bin/env sh
trap '
    signal=$?;
    cleanup
    exit $signal;
' INT

cleanup() {
    mv Cargo.tmp.toml Cargo.toml 2>/dev/null
}

cp Cargo.toml Cargo.tmp.toml
cp Cargo.1.48.0.toml Cargo.toml
cat Cargo.toml
cargo build --release
cleanup
