alias b := build
alias c := check
alias t := test

default:
  @just --list

# run `cargo build` on everything
build *ARGS="--workspace --all-targets":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo build {{ARGS}}

# run `cargo check` on everything
check *ARGS="--workspace --all-targets":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo check {{ARGS}}

# run code formatters
format:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo fmt --all
  nixpkgs-fmt $(echo **.nix)

# run tests
test: build
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo test

# run `cargo clippy` on everything
clippy *ARGS="--locked --offline --workspace --all-targets":
  cargo clippy {{ARGS}}

# run `cargo clippy --fix` on everything
clippy-fix *ARGS="--locked --offline --workspace --all-targets":
  cargo clippy {{ARGS}} --fix

# fix all typos
[no-exit-message]
typos-fix:
  just typos -w
