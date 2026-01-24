#!/bin/bash

set -x
set -euo pipefail

# Pin dependencies for MSRV

# To pin deps, switch toolchain to MSRV and execute the below updates

# cargo clean
# rustup override set 1.85.0

cargo update -p home --precise "0.5.11"
cargo update -p time --precise "0.3.45"
cargo update -p time-core --precise "0.1.7"
