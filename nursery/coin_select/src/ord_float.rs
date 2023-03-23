//! Newtypes around `f32` and `f64` that implement `Ord`.
//!
//! Backported from rust std lib [`total_cmp`] in version 1.62.0. Hopefully some day rust has this
//! in core: https://github.com/rust-lang/rfcs/issues/1249
//!
//! [`total_cmp`]: https://doc.rust-lang.org/core/primitive.f32.html#method.total_cmp

/// Wrapper for `f32` that implements `Ord`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ordf32(pub f32);
/// Wrapper for `f64` that implements `Ord`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ordf64(pub f64);

impl Ord for Ordf32 {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let mut left = self.0.to_bits() as i32;
        let mut right = other.0.to_bits() as i32;
        left ^= (((left >> 31) as u32) >> 1) as i32;
        right ^= (((right >> 31) as u32) >> 1) as i32;
        left.cmp(&right)
    }
}

impl Ord for Ordf64 {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let mut left = self.0.to_bits() as i64;
        let mut right = other.0.to_bits() as i64;
        left ^= (((left >> 63) as u64) >> 1) as i64;
        right ^= (((right >> 63) as u64) >> 1) as i64;
        left.cmp(&right)
    }
}

impl Eq for Ordf64 {}
impl Eq for Ordf32 {}

impl PartialOrd for Ordf32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialOrd for Ordf64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl core::fmt::Display for Ordf32 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl core::fmt::Display for Ordf64 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
