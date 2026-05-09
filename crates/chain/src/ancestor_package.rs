use bitcoin::{Amount, FeeRate, Weight};

/// Aggregated fee and weight for an unconfirmed ancestor chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AncestorPackage {
    /// Total weight of all unconfirmed transactions in the package.
    pub weight: Weight,
    /// Total fee of all unconfirmed transactions in the package.
    pub fee: Amount,
}

impl AncestorPackage {
    /// Create a new [`AncestorPackage`].
    pub fn new(weight: Weight, fee: Amount) -> Self {
        Self { weight, fee }
    }

    /// The additional fee a child transaction must contribute so that
    /// the package feerate reaches `target_feerate`.
    ///
    /// Returns [`Amount::ZERO`] if the package already meets or exceeds
    /// the target.
    pub fn fee_deficit(&self, target_feerate: FeeRate) -> Amount {
        let required = target_feerate * self.weight;
        required.checked_sub(self.fee).unwrap_or(Amount::ZERO)
    }
}
