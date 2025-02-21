#!/bin/bash

set -x
set -euo pipefail

# Pin dependencies for MSRV

# To pin deps, switch toolchain to MSRV and execute the below updates

# cargo clean
# rustup override set 1.63.0
cargo update -p tokio --precise "1.38.1"
cargo update -p tokio-util --precise "0.7.11"
cargo update -p home --precise "0.5.5"
cargo update -p regex --precise "1.7.3"
cargo update -p security-framework-sys --precise "2.11.1"
cargo update -p url --precise "2.5.0"
cargo update -p rustls@0.23.23 --precise "0.23.19"
cargo update -p hashbrown@0.15.2 --precise "0.15.0"
cargo update -p ureq --precise "2.10.1"