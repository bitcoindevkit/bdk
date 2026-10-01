//! Helpers shared by the fuzz targets.
//!
//! - [`chain`]: generators and invariant checks for the `bdk_chain` targets.
//! - [`fuzz_main`]: generates the fuzzing entry point for the enabled engine feature.

pub mod chain;
pub mod engines;
