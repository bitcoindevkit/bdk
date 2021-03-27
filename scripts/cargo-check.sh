#!/bin/bash
#
# Run various invocations of cargo check

features=( "default" "compiler" "electrum" "esplora" "compact_filters" "key-value-db" "async-interface" "all-keys" "keys-bip39" )
toolchains=( "+stable" "+1.46" "+nightly" )

main() {
    check_src
    check_all_targets
}

# Check with all features, with various toolchains.
check_src() {
    for toolchain in "${toolchains[@]}"; do
        cmd="cargo $toolchain clippy --all-targets --no-default-features"

        for feature in "${features[@]}"; do
            touch_files
            $cmd --features "$feature"
        done
    done
}

# Touch files to prevent cached warnings from not showing up.
touch_files() {
    touch $(find . -name *.rs)
}

main
exit 0
