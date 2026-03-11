#!/bin/bash

set -x
set -euo pipefail

# Pin dependencies for MSRV (1.63.0)

# To pin deps, switch toolchain to MSRV and execute the below updates

# cargo clean
# rustup override set 1.63.0

cargo update -p zstd-sys --precise "2.0.8+zstd.1.5.5"
cargo update -p time --precise "0.3.20"
cargo update -p home --precise "0.5.5"
cargo update -p proptest --precise "1.2.0"
cargo update -p url --precise "2.5.0"
cargo update -p tokio --precise "1.38.1"
cargo update -p reqwest --precise "0.12.4"
cargo update -p native-tls --precise "0.2.13"
cargo update -p security-framework-sys --precise "2.11.1"
cargo update -p csv --precise "1.3.0"
cargo update -p unicode-width --precise "0.1.13"
cargo update -p flate2 --precise "1.0.35"
cargo update -p bzip2-sys --precise "0.1.12"
cargo update -p ring --precise "0.17.12"
cargo update -p once_cell --precise "1.20.3"
cargo update -p base64ct --precise "1.6.0"
cargo update -p minreq --precise "2.13.2"
cargo update -p tracing --precise "0.1.40"
cargo update -p tracing-core --precise "0.1.33"
cargo update -p "webpki-roots@1.0.6" --precise "1.0.1"
cargo update -p rayon --precise "1.10.0"
cargo update -p rayon-core --precise "1.12.1"
cargo update -p quote --precise "1.0.41"
cargo update -p syn --precise "2.0.106"
cargo update -p openssl --precise "0.10.73"
cargo update -p openssl-sys --precise "0.9.109"
cargo update -p "getrandom@0.4.2" --precise "0.2.17"
cargo update -p serde_json --precise "1.0.138"
cargo update -p ryu --precise "1.0.18"
cargo update -p futures --precise "0.3.30"
cargo update -p futures-executor --precise "0.3.31"
cargo update -p futures-util --precise "0.3.31"
cargo update -p futures-macro --precise "0.3.31"
cargo update -p futures-channel --precise "0.3.31"
cargo update -p futures-core --precise "0.3.31"
cargo update -p futures-io --precise "0.3.31"
cargo update -p futures-sink --precise "0.3.31"
cargo update -p futures-task --precise "0.3.31"
cargo update -p proc-macro2 --precise "1.0.92"
cargo update -p log --precise "0.4.22"
cargo update -p itoa --precise "1.0.11"
cargo update -p anyhow --precise "1.0.86"
cargo update -p unicode-ident --precise "1.0.13"
cargo update -p hyper-util --precise "0.1.6"
cargo update -p pin-project --precise "1.1.5"
cargo update -p pin-project-internal --precise "1.1.5"
cargo update -p "rustls@0.23.37" --precise "0.23.26"
