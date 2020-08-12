use miniscript::{MiniscriptKey, Satisfier};

// De-facto standard "dust limit" (even though it should change based on the output type)
const DUST_LIMIT_SATOSHI: u64 = 546;

// we implement this trait to make sure we don't mess up the comparison with off-by-one like a <
// instead of a <= etc. The constant value for the dust limit is not public on purpose, to
// encourage the usage of this trait.
pub trait IsDust {
    fn is_dust(&self) -> bool;
}

impl IsDust for u64 {
    fn is_dust(&self) -> bool {
        *self <= DUST_LIMIT_SATOSHI
    }
}

#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
// Internally stored as satoshi/vbyte
pub struct FeeRate(f32);

impl FeeRate {
    pub fn from_btc_per_kvb(btc_per_kvb: f32) -> Self {
        FeeRate(btc_per_kvb * 1e5)
    }

    pub fn from_sat_per_vb(sat_per_vb: f32) -> Self {
        FeeRate(sat_per_vb)
    }

    pub fn default_min_relay_fee() -> Self {
        FeeRate(1.0)
    }

    pub fn as_sat_vb(&self) -> f32 {
        self.0
    }
}

impl std::default::Default for FeeRate {
    fn default() -> Self {
        FeeRate::default_min_relay_fee()
    }
}

pub struct After {
    pub current_height: Option<u32>,
    pub assume_height_reached: bool,
}

impl After {
    pub(crate) fn new(current_height: Option<u32>, assume_height_reached: bool) -> After {
        After {
            current_height,
            assume_height_reached,
        }
    }
}

impl<Pk: MiniscriptKey> Satisfier<Pk> for After {
    fn check_after(&self, n: u32) -> bool {
        if let Some(current_height) = self.current_height {
            current_height >= n
        } else {
            self.assume_height_reached
        }
    }
}

pub struct Older {
    pub current_height: Option<u32>,
    pub create_height: Option<u32>,
    pub assume_height_reached: bool,
}

impl Older {
    pub(crate) fn new(
        current_height: Option<u32>,
        create_height: Option<u32>,
        assume_height_reached: bool,
    ) -> Older {
        Older {
            current_height,
            create_height,
            assume_height_reached,
        }
    }
}

impl<Pk: MiniscriptKey> Satisfier<Pk> for Older {
    fn check_older(&self, n: u32) -> bool {
        if let Some(current_height) = self.current_height {
            // TODO: test >= / >
            current_height as u64 >= self.create_height.unwrap_or(0) as u64 + n as u64
        } else {
            self.assume_height_reached
        }
    }
}

pub struct ChunksIterator<I: Iterator> {
    iter: I,
    size: usize,
}

impl<I: Iterator> ChunksIterator<I> {
    pub fn new(iter: I, size: usize) -> Self {
        ChunksIterator { iter, size }
    }
}

impl<I: Iterator> Iterator for ChunksIterator<I> {
    type Item = Vec<<I as std::iter::Iterator>::Item>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut v = Vec::new();
        for _ in 0..self.size {
            let e = self.iter.next();

            match e {
                None => break,
                Some(val) => v.push(val),
            }
        }

        if v.is_empty() {
            return None;
        }

        Some(v)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_fee_from_btc_per_kb() {
        let fee = FeeRate::from_btc_per_kvb(1e-5);
        assert!((fee.as_sat_vb() - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_fee_from_sats_vbyte() {
        let fee = FeeRate::from_sat_per_vb(1.0);
        assert!((fee.as_sat_vb() - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_fee_default_min_relay_fee() {
        let fee = FeeRate::default_min_relay_fee();
        assert!((fee.as_sat_vb() - 1.0).abs() < 0.0001);
    }
}
