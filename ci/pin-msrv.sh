#!/bin/bash

set -x
set -euo pipefail

# Pin dependencies for MSRV

# To pin deps, switch toolchain to MSRV and execute the below updates

# cargo clean
# rustup default 1.63.0

cargo update -p zstd-sys --precise "2.0.8+zstd.1.5.5"
cargo update -p time --precise "0.3.20"
cargo update -p home --precise "0.5.5"
cargo update -p proptest --precise "1.2.0"
cargo update -p url --precise "2.5.0"
cargo update -p cc --precise "1.0.105"
cargo update -p tokio --precise "1.38.1"
cargo update -p tokio-util --precise "0.7.11"
cargo update -p indexmap --precise "2.5.0"
cargo update -p security-framework-sys --precise "2.11.1"
cargo update -p csv --precise "1.3.0"
cargo update -p unicode-width --precise "0.1.13"
