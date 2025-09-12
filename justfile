alias b := build
alias c := check
alias f := fmt
alias t := test
alias p := pre-push
alias vs := verify-standalone

_default:
  @just --list

# Build the project
build:
   cargo build

# Check code: formatting, compilation, linting, and commit signature
check:
   cargo +nightly fmt --all -- --check
   cargo check --workspace --all-features
   cargo clippy --all-features --all-targets -- -D warnings
   @[ "$(git log --pretty='format:%G?' -1 HEAD)" = "N" ] && \
       echo "\n⚠️  Unsigned commit: BDK requires that commits be signed." || \
       true

# Format all code
fmt:
   cargo +nightly fmt

# Run all tests for all crates with all features enabled
test:
   @just _test-bitcoind_rpc
   @just _test-chain
   @just _test-core
   @just _test-electrum
   @just _test-esplora
   @just _test-file_store
   @just _test-testenv

_test-bitcoind_rpc:
    cargo test -p bdk_bitcoind_rpc --all-features

_test-chain:
    cargo test -p bdk_chain --all-features

_test-core:
    cargo test -p bdk_core --all-features

_test-electrum:
    cargo test -p bdk_electrum --all-features

_test-esplora:
    cargo test -p bdk_esplora --all-features

_test-file_store:
    cargo test -p bdk_file_store --all-features

_test-testenv:
    cargo test -p bdk_testenv --all-features

# Run pre-push suite: format, check, and test
pre-push: fmt check test

# Verify that a crate can be built standalone without workspace dependencies
# This ensures the crate is publishable and doesn't accidentally depend on
# breaking changes from other workspace crates
verify-standalone crate *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    
    echo "Verifying {{crate}} can build standalone..."
    
    # Package the crate (default: --no-verify, can be overridden with args)
    cargo package -p {{crate}} --no-verify {{args}}
    
    # Find the packaged tarball
    CRATE_VERSION=$(cargo metadata --format-version 1 --no-deps | jq -r ".packages[] | select(.name == \"{{crate}}\") | .version")
    TARBALL="target/package/{{crate}}-${CRATE_VERSION}.crate"
    
    if [ ! -f "$TARBALL" ]; then
        echo "Error: Could not find packaged tarball at $TARBALL"
        exit 1
    fi
    
    # Create a temporary directory for unpacking
    TEMP_DIR=$(mktemp -d)
    trap "rm -rf $TEMP_DIR" EXIT
    
    # Unpack the tarball
    tar -xzf "$TARBALL" -C "$TEMP_DIR"
    
    # Build the unpacked crate with --locked to ensure it uses registry dependencies
    cd "$TEMP_DIR/{{crate}}-${CRATE_VERSION}"
    
    # Set temporary directories to avoid using workspace dependencies
    export CARGO_HOME="$TEMP_DIR/.cargo"
    export CARGO_TARGET_DIR="$TEMP_DIR/target"
    
    echo "Building {{crate}} in isolation..."
    cargo build --locked
    
    echo "✅ {{crate}} builds successfully in isolation!"
