// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Cross-platform time
//!
//! This module provides a function to get the current timestamp that works on all the platforms
//! supported by the library.
//!
//! It can be useful to compare it with the timestamps found in
//! [`TransactionDetails`](crate::types::TransactionDetails).

use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use js_sys::Date;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Instant as SystemInstant, SystemTime, UNIX_EPOCH};

/// Return the current timestamp in seconds
#[cfg(not(target_arch = "wasm32"))]
pub fn get_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
/// Return the current timestamp in seconds
#[cfg(target_arch = "wasm32")]
pub fn get_timestamp() -> u64 {
    let millis = Date::now();

    (millis / 1000.0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Instant(SystemInstant);
#[cfg(target_arch = "wasm32")]
pub(crate) struct Instant(Duration);

impl Instant {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Self {
        Instant(SystemInstant::now())
    }
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Self {
        let millis = Date::now();

        let secs = millis / 1000.0;
        let nanos = (millis % 1000.0) * 1e6;

        Instant(Duration::new(secs as u64, nanos as u32))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
    #[cfg(target_arch = "wasm32")]
    pub fn elapsed(&self) -> Duration {
        let now = Instant::new();

        now.0.checked_sub(self.0).unwrap_or(Duration::new(0, 0))
    }
}
