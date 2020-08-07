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

#[derive(Debug, Copy, Clone)]
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
