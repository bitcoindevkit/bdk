use crate::{
    bitcoin::{secp256k1::Secp256k1, ScriptBuf},
    miniscript::{Descriptor, DescriptorPublicKey},
};
use core::{borrow::Borrow, ops::Bound, ops::RangeBounds};

/// Maximum [BIP32](https://bips.xyz/32) derivation index.
pub const BIP32_MAX_INDEX: u32 = (1 << 31) - 1;

/// An iterator for derived script pubkeys.
///
/// [`SpkIterator`] is an implementation of the [`Iterator`] trait which possesses its own `next()`
/// and `nth()` functions, both of which circumvent the unnecessary intermediate derivations required
/// when using their default implementations.
///
/// ## Examples
///
/// ```
/// use bdk_chain::SpkIterator;
/// # use miniscript::{Descriptor, DescriptorPublicKey};
/// # use bitcoin::{secp256k1::Secp256k1};
/// # use std::str::FromStr;
/// # let secp = bitcoin::secp256k1::Secp256k1::signing_only();
/// # let (descriptor, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0)").unwrap();
/// # let external_spk_0 = descriptor.at_derivation_index(0).unwrap().script_pubkey();
/// # let external_spk_3 = descriptor.at_derivation_index(3).unwrap().script_pubkey();
/// # let external_spk_4 = descriptor.at_derivation_index(4).unwrap().script_pubkey();
///
/// // Creates a new script pubkey iterator starting at 0 from a descriptor.
/// let mut spk_iter = SpkIterator::new(&descriptor);
/// assert_eq!(spk_iter.next(), Some((0, external_spk_0)));
/// assert_eq!(spk_iter.next(), None);
/// ```
#[derive(Clone)]
pub struct SpkIterator<D> {
    next_index: u32,
    end: u32,
    descriptor: D,
    secp: Secp256k1<bitcoin::secp256k1::VerifyOnly>,
}

impl<D> SpkIterator<D>
where
    D: Borrow<Descriptor<DescriptorPublicKey>>,
{
    /// Creates a new script pubkey iterator starting at 0 from a descriptor.
    pub fn new(descriptor: D) -> Self {
        SpkIterator::new_with_range(descriptor, 0..=BIP32_MAX_INDEX)
    }

    // Creates a new script pubkey iterator from a descriptor with a given range.
    // If the descriptor doesn't have a wildcard, we shorten whichever range you pass in
    // to have length <= 1. This means that if you pass in 0..0 or 0..1 the range will
    // remain the same, but if you pass in 0..10, we'll shorten it to 0..1
    pub(crate) fn new_with_range<R>(descriptor: D, range: R) -> Self
    where
        R: RangeBounds<u32>,
    {
        let start = match range.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => *start + 1,
            Bound::Unbounded => u32::MIN,
        };

        let mut end = match range.end_bound() {
            Bound::Included(end) => *end + 1,
            Bound::Excluded(end) => *end,
            Bound::Unbounded => u32::MAX,
        };

        // Because `end` is exclusive, we want the maximum value to be BIP32_MAX_INDEX + 1.
        end = end.min(BIP32_MAX_INDEX + 1);

        if !descriptor.borrow().has_wildcard() {
            // The length of the range should be at most 1
            if end != start {
                end = start + 1;
            }
        }

        Self {
            next_index: start,
            end,
            descriptor,
            secp: Secp256k1::verification_only(),
        }
    }
}

impl<D> Iterator for SpkIterator<D>
where
    D: Borrow<Descriptor<DescriptorPublicKey>>,
{
    type Item = (u32, ScriptBuf);

    fn next(&mut self) -> Option<Self::Item> {
        // For non-wildcard descriptors, we expect the first element to be Some((0, spk)), then None after.
        // For wildcard descriptors, we expect it to keep iterating until exhausted.
        if self.next_index >= self.end {
            return None;
        }

        let script = self
            .descriptor
            .borrow()
            .derived_descriptor(&self.secp, self.next_index)
            .expect("the descriptor cannot need hardened derivation")
            .script_pubkey();
        let output = (self.next_index, script);

        self.next_index += 1;

        Some(output)
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.next_index = self
            .next_index
            .saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
        self.next()
    }
}

#[cfg(test)]
mod test {
    use crate::{
        bitcoin::secp256k1::Secp256k1,
        keychain::KeychainTxOutIndex,
        miniscript::{Descriptor, DescriptorPublicKey},
        spk_iter::{SpkIterator, BIP32_MAX_INDEX},
    };

    #[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
    enum TestKeychain {
        External,
        Internal,
    }

    fn init_txout_index() -> (
        KeychainTxOutIndex<TestKeychain>,
        Descriptor<DescriptorPublicKey>,
        Descriptor<DescriptorPublicKey>,
    ) {
        let mut txout_index = KeychainTxOutIndex::<TestKeychain>::default();

        let secp = Secp256k1::signing_only();
        let (external_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
        let (internal_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)").unwrap();

        txout_index.add_keychain(TestKeychain::External, external_descriptor.clone());
        txout_index.add_keychain(TestKeychain::Internal, internal_descriptor.clone());

        (txout_index, external_descriptor, internal_descriptor)
    }

    #[test]
    #[allow(clippy::iter_nth_zero)]
    #[rustfmt::skip]
    fn test_spkiterator_wildcard() {
        let (_, external_desc, _) = init_txout_index();
        let external_spk_0 = external_desc.at_derivation_index(0).unwrap().script_pubkey();
        let external_spk_16 = external_desc.at_derivation_index(16).unwrap().script_pubkey();
        let external_spk_20 = external_desc.at_derivation_index(20).unwrap().script_pubkey();
        let external_spk_21 = external_desc.at_derivation_index(21).unwrap().script_pubkey();
        let external_spk_max = external_desc.at_derivation_index(BIP32_MAX_INDEX).unwrap().script_pubkey();

        let mut external_spk = SpkIterator::new(&external_desc);
        let max_index = BIP32_MAX_INDEX - 22;

        assert_eq!(external_spk.next().unwrap(), (0, external_spk_0));
        assert_eq!(external_spk.nth(15).unwrap(), (16, external_spk_16));
        assert_eq!(external_spk.nth(3).unwrap(), (20, external_spk_20.clone()));
        assert_eq!(external_spk.next().unwrap(), (21, external_spk_21));
        assert_eq!(
            external_spk.nth(max_index as usize).unwrap(),
            (BIP32_MAX_INDEX, external_spk_max)
        );
        assert_eq!(external_spk.nth(0), None);

        let mut external_spk = SpkIterator::new_with_range(&external_desc, 0..21);
        assert_eq!(external_spk.nth(20).unwrap(), (20, external_spk_20));
        assert_eq!(external_spk.next(), None);

        let mut external_spk = SpkIterator::new_with_range(&external_desc, 0..21);
        assert_eq!(external_spk.nth(21), None);
    }

    #[test]
    #[allow(clippy::iter_nth_zero)]
    fn test_spkiterator_non_wildcard() {
        let secp = bitcoin::secp256k1::Secp256k1::signing_only();
        let (no_wildcard_descriptor, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0)").unwrap();
        let external_spk_0 = no_wildcard_descriptor
            .at_derivation_index(0)
            .unwrap()
            .script_pubkey();

        let mut external_spk = SpkIterator::new(&no_wildcard_descriptor);

        assert_eq!(external_spk.next().unwrap(), (0, external_spk_0.clone()));
        assert_eq!(external_spk.next(), None);

        let mut external_spk = SpkIterator::new(&no_wildcard_descriptor);

        assert_eq!(external_spk.nth(0).unwrap(), (0, external_spk_0.clone()));
        assert_eq!(external_spk.nth(0), None);

        let mut external_spk = SpkIterator::new_with_range(&no_wildcard_descriptor, 0..0);

        assert_eq!(external_spk.next(), None);

        let mut external_spk = SpkIterator::new_with_range(&no_wildcard_descriptor, 0..1);

        assert_eq!(external_spk.nth(0).unwrap(), (0, external_spk_0.clone()));
        assert_eq!(external_spk.next(), None);

        // We test that using new_with_range with range_len > 1 gives back an iterator with
        // range_len = 1
        let mut external_spk = SpkIterator::new_with_range(&no_wildcard_descriptor, 0..10);

        assert_eq!(external_spk.nth(0).unwrap(), (0, external_spk_0));
        assert_eq!(external_spk.nth(0), None);
    }

    // The following dummy traits were created to test if SpkIterator is working properly.
    trait TestSendStatic: Send + 'static {
        fn test(&self) -> u32 {
            20
        }
    }

    impl TestSendStatic for SpkIterator<Descriptor<DescriptorPublicKey>> {
        fn test(&self) -> u32 {
            20
        }
    }
}
