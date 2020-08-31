// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use js_sys::Date;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Instant as SystemInstant, SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
pub fn get_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
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
