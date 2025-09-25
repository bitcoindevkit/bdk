use bdk_core::CheckPoint;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use proptest::prelude::*;

/// Independent reference implementation of `bdk_core`'s private `skip_index` formula (mirrors
/// Bitcoin Core's `GetSkipHeight`, operating on 0-based checkpoint indices). Reimplementing the
/// formula in the test file keeps the structural-invariant assertion meaningful — if someone
/// changes the library's `skip_index`, this test will catch it instead of vacuously agreeing with
/// itself.
fn expected_skip_index(i: u32) -> u32 {
    if i < 2 {
        return 0;
    }
    fn invert_lowest_one(n: u32) -> u32 {
        n & n.wrapping_sub(1)
    }
    if i & 1 == 0 {
        invert_lowest_one(i)
    } else {
        invert_lowest_one(invert_lowest_one(i - 1)) + 1
    }
}

#[test]
fn test_skiplist_indices() {
    // Build a chain spanning multiple log levels and assert that each node carries the pskip
    // pointer mandated by the formula.
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());
    assert_eq!(cp.index(), 0);
    assert!(cp.skip().is_none(), "genesis must not have a skip pointer");

    const N: u32 = 5000;
    for height in 1..=N {
        let hash = BlockHash::from_byte_array([(height % 256) as u8; 32]);
        cp = cp.push(height, hash).unwrap();
        assert_eq!(cp.index(), height);
    }
    assert_eq!(cp.index(), N);

    // Walk the chain once and verify every non-genesis node skips to expected_skip_index(i).
    let mut current = cp;
    loop {
        let i = current.index();
        match current.skip() {
            Some(skip) => {
                let skip_index = skip.index();
                let exp_skip_index = expected_skip_index(i);
                assert_eq!(
                    skip_index, exp_skip_index,
                    "node at index {i} should skip to {exp_skip_index} but skips to {skip_index}",
                )
            }
            None => assert_eq!(i, 0, "only the genesis node may have no skip pointer"),
        }
        match current.prev() {
            Some(prev) => current = prev,
            None => break,
        }
    }
}

#[test]
fn test_skiplist_insert_maintains_indices() {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    // Build initial chain
    for height in [10, 20, 30, 40, 50] {
        let hash = BlockHash::from_byte_array([height as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Insert a block in the middle
    let hash = BlockHash::from_byte_array([25; 32]);
    cp = cp.insert(25, hash);

    // Check the full chain has correct indices
    let mut current = cp.clone();
    let expected_heights = [50, 40, 30, 25, 20, 10, 0];
    let expected_indices = [6, 5, 4, 3, 2, 1, 0];

    for (expected_height, expected_index) in expected_heights.iter().zip(expected_indices.iter()) {
        assert_eq!(current.height(), *expected_height);
        assert_eq!(current.index(), *expected_index);
        if *expected_height > 0 {
            current = current.prev().unwrap();
        }
    }
}

#[test]
fn test_range_edge_cases() {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    // Create sparse chain: 0, 100, 200, 300, 400, 500
    for i in 1..=5 {
        let height = i * 100;
        let hash = BlockHash::from_byte_array([i as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Empty range (start > end)
    #[allow(clippy::reversed_empty_ranges)]
    let empty: Vec<_> = cp.range(300..200).collect();
    assert!(empty.is_empty());

    // Single element range
    let single: Vec<_> = cp.range(300..=300).collect();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].height(), 300);

    // Range with non-existent bounds (150..250)
    let partial: Vec<_> = cp.range(150..250).collect();
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0].height(), 200);

    // Exclusive end bound (100..300 includes 100 and 200, but not 300)
    let exclusive: Vec<_> = cp.range(100..300).collect();
    assert_eq!(exclusive.len(), 2);
    assert_eq!(exclusive[0].height(), 200);
    assert_eq!(exclusive[1].height(), 100);

    // Unbounded range (..)
    let all: Vec<_> = cp.range(..).collect();
    assert_eq!(all.len(), 6);
    assert_eq!(all.first().unwrap().height(), 500);
    assert_eq!(all.last().unwrap().height(), 0);

    // Range beyond chain bounds
    let beyond: Vec<_> = cp.range(600..700).collect();
    assert!(beyond.is_empty());

    // Range from genesis
    let from_genesis: Vec<_> = cp.range(0..=200).collect();
    assert_eq!(from_genesis.len(), 3);
    assert_eq!(from_genesis[0].height(), 200);
    assert_eq!(from_genesis[2].height(), 0);
}

/// Build a sparse chain at the given heights (genesis at 0 is implicit; `heights` must be a
/// strictly increasing sequence of positive heights).
fn build_chain(heights: &[u32]) -> CheckPoint<BlockHash> {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());
    for (i, &h) in heights.iter().enumerate() {
        let hash = BlockHash::from_byte_array([((i + 1) & 0xff) as u8; 32]);
        cp = cp.push(h, hash).unwrap();
    }
    cp
}

/// Linear-scan reference implementations against which the pskip-accelerated `get`/`range`/
/// `floor_at` are compared. Walks the chain via `prev()` only.
mod reference {
    use super::*;
    use core::ops::{Bound, RangeBounds};

    pub(super) fn get(cp: &CheckPoint<BlockHash>, height: u32) -> Option<CheckPoint<BlockHash>> {
        cp.iter().find(|c| c.height() == height)
    }

    pub(super) fn floor_at(
        cp: &CheckPoint<BlockHash>,
        height: u32,
    ) -> Option<CheckPoint<BlockHash>> {
        cp.iter().find(|c| c.height() <= height)
    }

    pub(super) fn range_heights<R: RangeBounds<u32>>(
        cp: &CheckPoint<BlockHash>,
        range: R,
    ) -> Vec<u32> {
        let lo = match range.start_bound() {
            Bound::Included(&v) => v,
            Bound::Excluded(&v) => v.saturating_add(1),
            Bound::Unbounded => 0,
        };
        let hi = match range.end_bound() {
            Bound::Included(&v) => v,
            Bound::Excluded(&v) => v.saturating_sub(1),
            Bound::Unbounded => u32::MAX,
        };
        if lo > hi {
            return vec![];
        }
        cp.iter()
            .filter(|c| c.height() >= lo && c.height() <= hi)
            .map(|c| c.height())
            .collect()
    }
}

/// Strategy: pick a small, strictly-increasing set of positive heights up to a bound.
fn arbitrary_sparse_heights() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(1u32..=10_000, 0..200).prop_map(|mut v| {
        v.sort_unstable();
        v.dedup();
        v
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// `get(h)` matches a linear scan for any chain and any query height (existing, missing,
    /// genesis, beyond tip, etc.).
    #[test]
    fn pskip_get_matches_linear(heights in arbitrary_sparse_heights(), q in 0u32..=10_500) {
        let cp = build_chain(&heights);
        let expected = reference::get(&cp, q).map(|c| c.height());
        let got = cp.get(q).map(|c| c.height());
        prop_assert_eq!(got, expected);
    }

    /// `floor_at(h)` matches a linear scan.
    #[test]
    fn pskip_floor_at_matches_linear(heights in arbitrary_sparse_heights(), q in 0u32..=10_500) {
        let cp = build_chain(&heights);
        let expected = reference::floor_at(&cp, q).map(|c| c.height());
        let got = cp.floor_at(q).map(|c| c.height());
        prop_assert_eq!(got, expected);
    }

    /// `range(a..=b)` (inclusive) matches a linear scan.
    #[test]
    fn pskip_range_inclusive_matches_linear(
        heights in arbitrary_sparse_heights(),
        a in 0u32..=10_500,
        b in 0u32..=10_500,
    ) {
        let cp = build_chain(&heights);
        let expected = reference::range_heights(&cp, a..=b);
        let got: Vec<u32> = cp.range(a..=b).map(|c| c.height()).collect();
        prop_assert_eq!(got, expected);
    }

    /// `range(a..b)` (exclusive end) matches a linear scan.
    #[test]
    fn pskip_range_exclusive_matches_linear(
        heights in arbitrary_sparse_heights(),
        a in 0u32..=10_500,
        b in 0u32..=10_500,
    ) {
        let cp = build_chain(&heights);
        let expected = reference::range_heights(&cp, a..b);
        let got: Vec<u32> = cp.range(a..b).map(|c| c.height()).collect();
        prop_assert_eq!(got, expected);
    }

    /// `range(..=b)` (unbounded start) matches a linear scan.
    #[test]
    fn pskip_range_unbounded_start_matches_linear(
        heights in arbitrary_sparse_heights(),
        b in 0u32..=10_500,
    ) {
        let cp = build_chain(&heights);
        let expected = reference::range_heights(&cp, ..=b);
        let got: Vec<u32> = cp.range(..=b).map(|c| c.height()).collect();
        prop_assert_eq!(got, expected);
    }

    /// `range(a..)` (unbounded end) matches a linear scan.
    #[test]
    fn pskip_range_unbounded_end_matches_linear(
        heights in arbitrary_sparse_heights(),
        a in 0u32..=10_500,
    ) {
        let cp = build_chain(&heights);
        let expected = reference::range_heights(&cp, a..);
        let got: Vec<u32> = cp.range(a..).map(|c| c.height()).collect();
        prop_assert_eq!(got, expected);
    }
}
