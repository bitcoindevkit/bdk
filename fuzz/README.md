# Fuzzing

Fuzz targets for the BDK crates. All commands run from this directory (`fuzz/`).

Targets are grouped per crate: harnesses live in `fuzz_targets/<crate>/` and the
generators and invariant checks they share live in `src/<crate>/`. Engine plumbing
(`src/engines.rs`) is shared by every target.

## Targets

### `bdk_chain`

| Target                             | What it fuzzes                                          |
| ---------------------------------- | ------------------------------------------------------- |
| `local_chain_apply_update`         | `LocalChain<BlockHash>`                                  |
| `local_chain_apply_update_header`  | `LocalChain<Header>` (checkpoint gaps become placeholders) |

Each target drives a chain through a sequence of arbitrary operations
(`apply_update`, `apply_changeset`, `insert_block`, `disconnect_from`,
`apply_header{,_connected_to}`) and asserts the invariants in
`src/chain/checks.rs` after every one.

## libFuzzer

Requires nightly.

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run local_chain_apply_update --features libfuzzer_fuzz -- -max_total_time=300
```

Crashes land in `artifacts/<target>/`. Replay one with:

```sh
cargo +nightly fuzz run local_chain_apply_update --features libfuzzer_fuzz artifacts/local_chain_apply_update/crash-<hash>
```

## honggfuzz

```sh
sudo apt-get install -y binutils-dev libunwind-dev   # build dependencies
cargo install honggfuzz

HFUZZ_BUILD_ARGS="--features honggfuzz_fuzz" \
HFUZZ_RUN_ARGS="--run_time 300 --exit_upon_crash -v" \
    cargo hfuzz run local_chain_apply_update
```

A crash writes `hfuzz_workspace/<target>/HONGGFUZZ.REPORT.TXT`.

## AFL++

```sh
cargo install cargo-afl
cargo afl config --build

mkdir -p afl-seeds && printf 'bdk-fuzz-seed' > afl-seeds/seed
cargo afl build --features afl_fuzz --bin local_chain_apply_update
cargo afl fuzz -i afl-seeds -o afl-out -V 300 -- target/debug/local_chain_apply_update
```

Crashes land in `afl-out/*/crashes/`.

## Replaying without a fuzzer

Built with no engine feature, each target gets a `main` that replays the corpus
files passed as arguments. Useful for debugging a crash under `rust-gdb` or with
a backtrace:

```sh
cargo build --bin local_chain_apply_update
RUST_BACKTRACE=1 ./target/debug/local_chain_apply_update corpus/local_chain_apply_update/*
```

## Coverage report

`cargo fuzz coverage` runs a target over its corpus and writes
`coverage/<target>/coverage.profdata`. Report on that profile against the same
target's binary:

```sh
HOST=$(rustc +nightly -vV | sed -n 's/^host: //p')
LLVM_BIN="$(rustc +nightly --print sysroot)/lib/rustlib/$HOST/bin"
BUILD_DIR="target/$HOST/coverage/$HOST/release"
SOURCES="../crates/chain/src/local_chain.rs ../crates/core/src/checkpoint.rs"

for target in local_chain_apply_update local_chain_apply_update_header; do
    cargo +nightly fuzz coverage "$target" --features libfuzzer_fuzz

    echo "=== $target"
    "$LLVM_BIN/llvm-cov" report \
        -instr-profile="coverage/$target/coverage.profdata" \
        -object "$BUILD_DIR/$target" \
        -sources $SOURCES
done
```

Swap `report` for `show ... --format=html --output-dir=coverage/$target/report`
to get a line-by-line HTML report per target.

Requires the `llvm-tools` rustup component (`rustup component add llvm-tools`).
