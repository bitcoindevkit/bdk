# example_electrum (BDK)

This is a tiny command-line wallet demo that uses `bdk_electrum` to sync wallet state from an Electrum server.

## Quickstart (watch-only, signet)

1) Generate sample descriptors (prints **public** + **private** descriptors):

```bash
cargo run -p example_electrum -- generate -n signet
```

2) Pick the **Public** external descriptor (the `/0/*` line), then initialize a local data store:

```bash
# Replace with the public descriptor you generated
DESC="tr([FINGERPRINT/86'/1'/0']tpub.../0/*)#checksum"

cargo run -p example_electrum -- init "$DESC" -n signet
```

3) Sync from the default Electrum server for the selected network:

```bash
# One-liner sync (uses the default Electrum URL if you don’t pass one)
cargo run -p example_electrum -- sync --unused-spks
```

To use a specific Electrum server:

```bash
cargo run -p example_electrum -- sync --unused-spks tcp://signet-electrumx.wakiyamap.dev:50001
```

## Notes

- `sync` scans addresses/scripts and updates the local store.
- Use `scan` if you want a broader scan strategy (see `--help`).

```bash
cargo run -p example_electrum -- sync --help
cargo run -p example_electrum -- scan --help
```
