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
