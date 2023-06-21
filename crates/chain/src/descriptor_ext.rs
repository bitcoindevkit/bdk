use crate::miniscript::{Descriptor, DescriptorPublicKey};

/// A trait to extend the functionality of a miniscript descriptor.
pub trait DescriptorExt {
    /// Returns the minimum value (in satoshis) at which an output is broadcastable.
    /// Panics if the descriptor wildcard is hardened.
    fn dust_value(&self) -> u64;
}

impl DescriptorExt for Descriptor<DescriptorPublicKey> {
    fn dust_value(&self) -> u64 {
        self.at_derivation_index(0)
            .expect("descriptor can't have hardened derivation")
            .script_pubkey()
            .dust_value()
            .to_sat()
    }
}
