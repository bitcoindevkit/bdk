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
pub struct Instant(SystemInstant);
#[cfg(target_arch = "wasm32")]
pub struct Instant(Duration);

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
