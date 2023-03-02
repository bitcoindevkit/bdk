use super::*;

/// A [`WeightedValue`] represents an input candidate for [`CoinSelector`]. This can either be a
/// single UTXO, or a group of UTXOs that should be spent together.
#[derive(Debug, Clone, Copy)]
pub struct WeightedValue {
    /// Total value of the UTXO(s) that this [`WeightedValue`] represents.
    pub value: u64,
    /// Total weight of including this/these UTXO(s).
    /// `txin` fields: `prevout`, `nSequence`, `scriptSigLen`, `scriptSig`, `scriptWitnessLen`,
    /// `scriptWitness` should all be included.
    pub weight: u32,
    /// Total number of inputs; so we can calculate extra `varint` weight due to `vin` len changes.
    pub input_count: usize,
    /// Whether this [`WeightedValue`] contains at least one segwit spend.
    pub is_segwit: bool,
}

impl WeightedValue {
    /// Create a new [`WeightedValue`] that represents a single input.
    ///
    /// `satisfaction_weight` is the weight of `scriptSigLen + scriptSig + scriptWitnessLen +
    /// scriptWitness`.
    pub fn new(value: u64, satisfaction_weight: u32, is_segwit: bool) -> WeightedValue {
        let weight = TXIN_BASE_WEIGHT + satisfaction_weight;
        WeightedValue {
            value,
            weight,
            input_count: 1,
            is_segwit,
        }
    }

    /// Effective value of this input candidate: `actual_value - input_weight * feerate (sats/wu)`.
    pub fn effective_value(&self, effective_feerate: f32) -> i64 {
        // We prefer undershooting the candidate's effective value (so we over estimate the fee of a
        // candidate). If we overshoot the candidate's effective value, it may be possible to find a
        // solution which does not meet the target feerate.
        self.value as i64 - (self.weight as f32 * effective_feerate).ceil() as i64
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CoinSelectorOpt {
    /// The value we need to select.
    /// If the value is `None` then the selection will be complete if it can pay for the drain
    /// output and satisfy the other constraints (e.g. minimum fees).
    pub target_value: Option<u64>,
    /// Additional leeway for the target value.
    pub max_extra_target: u64, // TODO: Maybe out of scope here?

    /// The feerate we should try and achieve in sats per weight unit.
    pub target_feerate: f32,
    /// The feerate
    pub long_term_feerate: Option<f32>, // TODO: Maybe out of scope? (waste)
    /// The minimum absolute fee. I.e. needed for RBF.
    pub min_absolute_fee: u64,

    /// The weight of the template transaction including fixed fields and outputs.
    pub base_weight: u32,
    /// Additional weight if we include the drain (change) output.
    pub drain_weight: u32,
    /// Weight of spending the drain (change) output in the future.
    pub spend_drain_weight: u32, // TODO: Maybe out of scope? (waste)

    /// Minimum value allowed for a drain (change) output.
    pub min_drain_value: u64,
}

impl CoinSelectorOpt {
    fn from_weights(base_weight: u32, drain_weight: u32, spend_drain_weight: u32) -> Self {
        // 0.25 sats/wu == 1 sat/vb
        let target_feerate = 0.25_f32;

        // set `min_drain_value` to dust limit
        let min_drain_value =
            3 * ((drain_weight + spend_drain_weight) as f32 * target_feerate) as u64;

        Self {
            target_value: None,
            max_extra_target: 0,
            target_feerate,
            long_term_feerate: None,
            min_absolute_fee: 0,
            base_weight,
            drain_weight,
            spend_drain_weight,
            min_drain_value,
        }
    }

    pub fn fund_outputs(
        txouts: &[TxOut],
        drain_output: &TxOut,
        drain_satisfaction_weight: u32,
    ) -> Self {
        let mut tx = Transaction {
            input: vec![],
            version: 1,
            lock_time: LockTime::ZERO.into(),
            output: txouts.to_vec(),
        };
        let base_weight = tx.weight();
        // this awkward calculation is necessary since TxOut doesn't have \.weight()
        let drain_weight = {
            tx.output.push(drain_output.clone());
            tx.weight() - base_weight
        };
        Self {
            target_value: if txouts.is_empty() {
                None
            } else {
                Some(txouts.iter().map(|txout| txout.value).sum())
            },
            ..Self::from_weights(
                base_weight as u32,
                drain_weight as u32,
                TXIN_BASE_WEIGHT + drain_satisfaction_weight,
            )
        }
    }

    pub fn long_term_feerate(&self) -> f32 {
        self.long_term_feerate.unwrap_or(self.target_feerate)
    }

    pub fn drain_waste(&self) -> i64 {
        (self.drain_weight as f32 * self.target_feerate
            + self.spend_drain_weight as f32 * self.long_term_feerate()) as i64
    }
}

/// [`CoinSelector`] is responsible for selecting and deselecting from a set of canididates.
#[derive(Debug, Clone)]
pub struct CoinSelector<'a> {
    pub opts: &'a CoinSelectorOpt,
    pub candidates: &'a Vec<WeightedValue>,
    selected: BTreeSet<usize>,
}

impl<'a> CoinSelector<'a> {
    pub fn candidate(&self, index: usize) -> &WeightedValue {
        &self.candidates[index]
    }

    pub fn new(candidates: &'a Vec<WeightedValue>, opts: &'a CoinSelectorOpt) -> Self {
        Self {
            candidates,
            selected: Default::default(),
            opts,
        }
    }

    pub fn select(&mut self, index: usize) -> bool {
        assert!(index < self.candidates.len());
        self.selected.insert(index)
    }

    pub fn deselect(&mut self, index: usize) -> bool {
        self.selected.remove(&index)
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Weight sum of all selected inputs.
    pub fn selected_weight(&self) -> u32 {
        self.selected
            .iter()
            .map(|&index| self.candidates[index].weight)
            .sum()
    }

    /// Effective value sum of all selected inputs.
    pub fn selected_effective_value(&self) -> i64 {
        self.selected
            .iter()
            .map(|&index| self.candidates[index].effective_value(self.opts.target_feerate))
            .sum()
    }

    /// Absolute value sum of all selected inputs.
    pub fn selected_absolute_value(&self) -> u64 {
        self.selected
            .iter()
            .map(|&index| self.candidates[index].value)
            .sum()
    }

    /// Waste sum of all selected inputs.
    pub fn selected_waste(&self) -> i64 {
        (self.selected_weight() as f32 * (self.opts.target_feerate - self.opts.long_term_feerate()))
            as i64
    }

    /// Current weight of template tx + selected inputs.
    pub fn current_weight(&self) -> u32 {
        let witness_header_extra_weight = self
            .selected()
            .find(|(_, wv)| wv.is_segwit)
            .map(|_| 2)
            .unwrap_or(0);
        let vin_count_varint_extra_weight = {
            let input_count = self.selected().map(|(_, wv)| wv.input_count).sum::<usize>();
            (varint_size(input_count) - 1) * 4
        };
        self.opts.base_weight
            + self.selected_weight()
            + witness_header_extra_weight
            + vin_count_varint_extra_weight
    }

    /// Current excess.
    pub fn current_excess(&self) -> i64 {
        self.selected_effective_value() - self.effective_target()
    }

    /// This is the effective target value.
    pub fn effective_target(&self) -> i64 {
        let (has_segwit, max_input_count) = self
            .candidates
            .iter()
            .fold((false, 0_usize), |(is_segwit, input_count), c| {
                (is_segwit || c.is_segwit, input_count + c.input_count)
            });

        let effective_base_weight = self.opts.base_weight
            + if has_segwit { 2_u32 } else { 0_u32 }
            + (varint_size(max_input_count) - 1) * 4;

        self.opts.target_value.unwrap_or(0) as i64
            + (effective_base_weight as f32 * self.opts.target_feerate).ceil() as i64
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn selected(&self) -> impl Iterator<Item = (usize, &'a WeightedValue)> + '_ {
        self.selected
            .iter()
            .map(move |&index| (index, &self.candidates[index]))
    }

    pub fn unselected(&self) -> impl Iterator<Item = (usize, &'a WeightedValue)> + '_ {
        self.candidates
            .iter()
            .enumerate()
            .filter(move |(index, _)| !self.selected.contains(index))
    }

    pub fn selected_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().cloned()
    }

    pub fn unselected_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.candidates.len()).filter(move |index| !self.selected.contains(index))
    }

    pub fn all_selected(&self) -> bool {
        self.selected.len() == self.candidates.len()
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.candidates.len()).collect();
    }

    pub fn select_until_finished(&mut self) -> Result<Selection, SelectionError> {
        let mut selection = self.finish();

        if selection.is_ok() {
            return selection;
        }

        let unselected = self.unselected_indexes().collect::<Vec<_>>();

        for index in unselected {
            self.select(index);
            selection = self.finish();

            if selection.is_ok() {
                break;
            }
        }

        selection
    }

    pub fn finish(&self) -> Result<Selection, SelectionError> {
        let weight_without_drain = self.current_weight();
        let weight_with_drain = weight_without_drain + self.opts.drain_weight;

        let fee_without_drain =
            (weight_without_drain as f32 * self.opts.target_feerate).ceil() as u64;
        let fee_with_drain = (weight_with_drain as f32 * self.opts.target_feerate).ceil() as u64;

        let inputs_minus_outputs = {
            let target_value = self.opts.target_value.unwrap_or(0);
            let selected = self.selected_absolute_value();

            // find the largest unsatisfied constraint (if any), and return error of that constraint
            // "selected" should always be greater than or equal to these selected values
            [
                (
                    SelectionConstraint::TargetValue,
                    target_value.saturating_sub(selected),
                ),
                (
                    SelectionConstraint::TargetFee,
                    (target_value + fee_without_drain).saturating_sub(selected),
                ),
                (
                    SelectionConstraint::MinAbsoluteFee,
                    (target_value + self.opts.min_absolute_fee).saturating_sub(selected),
                ),
                (
                    SelectionConstraint::MinDrainValue,
                    // when we have no target value (hence no recipient txouts), we need to ensure
                    // the selected amount can satisfy requirements for a drain output (so we at
                    // least have one txout)
                    if self.opts.target_value.is_none() {
                        (fee_with_drain + self.opts.min_drain_value).saturating_sub(selected)
                    } else {
                        0
                    },
                ),
            ]
            .iter()
            .filter(|&(_, v)| v > &0)
            .max_by_key(|&(_, v)| v)
            .map_or(Ok(()), |(constraint, missing)| {
                Err(SelectionError {
                    selected,
                    missing: *missing,
                    constraint: *constraint,
                })
            })?;

            (selected - target_value) as u64
        };

        let fee_without_drain = fee_without_drain.max(self.opts.min_absolute_fee);
        let fee_with_drain = fee_with_drain.max(self.opts.min_absolute_fee);

        let excess_without_drain = inputs_minus_outputs - fee_without_drain;
        let input_waste = self.selected_waste();

        // begin preparing excess strategies for final selection
        let mut excess_strategies = HashMap::new();

        // only allow `ToFee` and `ToRecipient` excess strategies when we have a `target_value`,
        // otherwise we will result in a result with no txouts, or attempt to add value to an output
        // that does not exist
        if self.opts.target_value.is_some() {
            // no drain, excess to fee
            excess_strategies.insert(
                ExcessStrategyKind::ToFee,
                ExcessStrategy {
                    recipient_value: self.opts.target_value,
                    drain_value: None,
                    fee: fee_without_drain + excess_without_drain,
                    weight: weight_without_drain,
                    waste: input_waste + excess_without_drain as i64,
                },
            );

            // no drain, excess to recipient
            // if `excess == 0`, this result will be the same as the previous, so don't consider it
            // if `max_extra_target == 0`, there is no leeway for this strategy
            if excess_without_drain > 0 && self.opts.max_extra_target > 0 {
                let extra_recipient_value =
                    core::cmp::min(self.opts.max_extra_target, excess_without_drain);
                let extra_fee = excess_without_drain - extra_recipient_value;
                excess_strategies.insert(
                    ExcessStrategyKind::ToRecipient,
                    ExcessStrategy {
                        recipient_value: self.opts.target_value.map(|v| v + extra_recipient_value),
                        drain_value: None,
                        fee: fee_without_drain + extra_fee,
                        weight: weight_without_drain,
                        waste: input_waste + extra_fee as i64,
                    },
                );
            }
        }

        // with drain
        if fee_with_drain >= self.opts.min_absolute_fee
            && inputs_minus_outputs >= fee_with_drain + self.opts.min_drain_value
        {
            excess_strategies.insert(
                ExcessStrategyKind::ToDrain,
                ExcessStrategy {
                    recipient_value: self.opts.target_value,
                    drain_value: Some(inputs_minus_outputs.saturating_sub(fee_with_drain)),
                    fee: fee_with_drain,
                    weight: weight_with_drain,
                    waste: input_waste + self.opts.drain_waste(),
                },
            );
        }

        debug_assert!(
            !excess_strategies.is_empty(),
            "should have at least one excess strategy"
        );

        Ok(Selection {
            selected: self.selected.clone(),
            excess: excess_without_drain,
            excess_strategies,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SelectionError {
    selected: u64,
    missing: u64,
    constraint: SelectionConstraint,
}

impl core::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SelectionError {
                selected,
                missing,
                constraint,
            } => write!(
                f,
                "insufficient coins selected; selected={}, missing={}, unsatisfied_constraint={:?}",
                selected, missing, constraint
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SelectionError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionConstraint {
    /// The target is not met
    TargetValue,
    /// The target fee (given the feerate) is not met
    TargetFee,
    /// Min absolute fee is not met
    MinAbsoluteFee,
    /// Min drain value is not met
    MinDrainValue,
}

impl core::fmt::Display for SelectionConstraint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SelectionConstraint::TargetValue => core::write!(f, "target_value"),
            SelectionConstraint::TargetFee => core::write!(f, "target_fee"),
            SelectionConstraint::MinAbsoluteFee => core::write!(f, "min_absolute_fee"),
            SelectionConstraint::MinDrainValue => core::write!(f, "min_drain_value"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub selected: BTreeSet<usize>,
    pub excess: u64,
    pub excess_strategies: HashMap<ExcessStrategyKind, ExcessStrategy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, core::hash::Hash)]
pub enum ExcessStrategyKind {
    ToFee,
    ToRecipient,
    ToDrain,
}

#[derive(Clone, Copy, Debug)]
pub struct ExcessStrategy {
    pub recipient_value: Option<u64>,
    pub drain_value: Option<u64>,
    pub fee: u64,
    pub weight: u32,
    pub waste: i64,
}

impl Selection {
    pub fn apply_selection<'a, T>(
        &'a self,
        candidates: &'a [T],
    ) -> impl Iterator<Item = &'a T> + 'a {
        self.selected.iter().map(move |i| &candidates[*i])
    }

    /// Returns the [`ExcessStrategy`] that results in the least waste.
    pub fn best_strategy(&self) -> (&ExcessStrategyKind, &ExcessStrategy) {
        self.excess_strategies
            .iter()
            .min_by_key(|&(_, a)| a.waste)
            .expect("selection has no excess strategy")
    }
}

impl core::fmt::Display for ExcessStrategyKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExcessStrategyKind::ToFee => core::write!(f, "to_fee"),
            ExcessStrategyKind::ToRecipient => core::write!(f, "to_recipient"),
            ExcessStrategyKind::ToDrain => core::write!(f, "to_drain"),
        }
    }
}

impl ExcessStrategy {
    /// Returns feerate in sats/wu.
    pub fn feerate(&self) -> f32 {
        self.fee as f32 / self.weight as f32
    }
}

#[cfg(test)]
mod test {
    use crate::{ExcessStrategyKind, SelectionConstraint};

    use super::{CoinSelector, CoinSelectorOpt, WeightedValue};

    /// Ensure `target_value` is respected. Can't have no disrespect.
    #[test]
    fn target_value_respected() {
        let target_value = 1000_u64;

        let candidates = (500..1500_u64)
            .map(|value| WeightedValue {
                value,
                weight: 100,
                input_count: 1,
                is_segwit: false,
            })
            .collect::<super::Vec<_>>();

        let opts = CoinSelectorOpt {
            target_value: Some(target_value),
            max_extra_target: 0,
            target_feerate: 0.00,
            long_term_feerate: None,
            min_absolute_fee: 0,
            base_weight: 10,
            drain_weight: 10,
            spend_drain_weight: 10,
            min_drain_value: 10,
        };

        for (index, v) in candidates.iter().enumerate() {
            let mut selector = CoinSelector::new(&candidates, &opts);
            assert!(selector.select(index));

            let res = selector.finish();
            if v.value < opts.target_value.unwrap_or(0) {
                let err = res.expect_err("should have failed");
                assert_eq!(err.selected, v.value);
                assert_eq!(err.missing, target_value - v.value);
                assert_eq!(err.constraint, SelectionConstraint::MinAbsoluteFee);
            } else {
                let sel = res.expect("should have succeeded");
                assert_eq!(sel.excess, v.value - opts.target_value.unwrap_or(0));
            }
        }
    }

    #[test]
    fn drain_all() {
        let candidates = (0..100)
            .map(|_| WeightedValue {
                value: 666,
                weight: 166,
                input_count: 1,
                is_segwit: false,
            })
            .collect::<super::Vec<_>>();

        let opts = CoinSelectorOpt {
            target_value: None,
            max_extra_target: 0,
            target_feerate: 0.25,
            long_term_feerate: None,
            min_absolute_fee: 0,
            base_weight: 10,
            drain_weight: 100,
            spend_drain_weight: 66,
            min_drain_value: 1000,
        };

        let selection = CoinSelector::new(&candidates, &opts)
            .select_until_finished()
            .expect("should succeed");

        assert!(selection.selected.len() > 1);
        assert_eq!(selection.excess_strategies.len(), 1);

        let (kind, strategy) = selection.best_strategy();
        assert_eq!(*kind, ExcessStrategyKind::ToDrain);
        assert!(strategy.recipient_value.is_none());
        assert!(strategy.drain_value.is_some());
    }

    /// TODO: Tests to add:
    /// * `finish` should ensure at least `target_value` is selected.
    /// * actual feerate should be equal or higher than `target_feerate`.
    /// * actual drain value should be equal or higher than `min_drain_value` (or else no drain).
    fn _todo() {}
}
