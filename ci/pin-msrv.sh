#!/bin/bash

set -x
set -euo pipefail

# Pin dependencies for MSRV

# To pin deps, switch toolchain to MSRV and execute the below updates

# cargo clean
# rustup override set 1.85.0

cargo update -p idna_adapter --precise "1.2.1"
cargo update -p icu_normalizer --precise "2.1.1"
cargo update -p icu_provider --precise "2.1.1"
cargo update -p icu_locale_core --precise "2.1.1"
