# BDK Common

`bdk_common` is an **internal-only** crate providing zero-overhead, feature-gated logging macros for
all BDK chain-sync crates. Enabling the single `log` feature pulls in `tracing = "0.1"`. When it’s
off, the macros compile to no-ops and there’s no runtime or dependency impact.

## Features

- **`log`** (off by default)
  – Re-exports `tracing` and enables logging.
  – When disabled, `log_trace!`, `log_debug!`, `log_info!`, `log_warn!`, `log_error!`, and `log_span!` expand to nothing.
