use bitcoin::{
    secp256k1::{Secp256k1, VerifyOnly},
    Script,
};
use miniscript::{Descriptor, DescriptorPublicKey};

/// An iterator over a descriptor's script pubkeys.
///
// TODO: put this into miniscript
#[derive(Clone, Debug)]
pub struct SpkIter {
    descriptor: Descriptor<DescriptorPublicKey>,
    index: usize,
    secp: Secp256k1<VerifyOnly>,
    end: usize,
}

impl SpkIter {
    /// Creates a new script pubkey iterator starting at 0 from a descriptor
    pub fn new(descriptor: Descriptor<DescriptorPublicKey>) -> Self {
        let secp = Secp256k1::verification_only();
        let end = if descriptor.has_wildcard() {
            // Because we only iterate over non-hardened indexes there are 2^31 values
            (1 << 31) - 1
        } else {
            0
        };

        Self {
            descriptor,
            index: 0,
            secp,
            end,
        }
    }
}

impl Iterator for SpkIter {
    type Item = (u32, Script);

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.index = self.index.saturating_add(n);
        self.next()
    }

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index;
        if index > self.end {
            return None;
        }

        let script = self
            .descriptor
            .at_derivation_index(self.index as u32)
            .derived_descriptor(&self.secp)
            .expect("the descritpor cannot need hardened derivation")
            .script_pubkey();

        self.index += 1;

        Some((index as u32, script))
    }
}
