use super::*;
#[allow(unused)] // some bug in <= 1.48.0 sees this as unused when it isn't
use crate::float::FloatExt;
use crate::{bnb::BnBMetric, float::Ordf32, FeeRate};
use alloc::{borrow::Cow, collections::BTreeSet, vec::Vec};

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
    pub fn effective_value(&self, feerate: FeeRate) -> Ordf32 {
        Ordf32(self.value as f32 - (self.weight as f32 * feerate.spwu()))
    }

    /// Value per weight unit
    pub fn value_pwu(&self) -> Ordf32 {
        Ordf32(self.value as f32 / self.weight as f32)
    }
}

/// A drain (A.K.A. change) output.
/// Technically it could represent multiple outputs.
///
/// These are usually created by a [`change_policy`].
///
/// [`change_policy`]: crate::change_policy
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Drain {
    /// The weight of adding this drain
    pub weight: u32,
    /// The value that should be assigned to the drain
    pub value: u64,
    /// The weight of spending this drain
    pub spend_weight: u32,
}

impl Drain {
    /// A drian representing no drain at all.
    pub fn none() -> Self {
        Self::default()
    }

    /// is the "none" drain
    pub fn is_none(&self) -> bool {
        self == &Drain::none()
    }

    /// Is not the "none" drain
    pub fn is_some(&self) -> bool {
        !self.is_none()
    }

    /// The waste of adding this drain to a transaction according to the [waste metric].
    ///
    /// [waste metric]; https://bitcoin.stackexchange.com/questions/113622/what-does-waste-metric-mean-in-the-context-of-coin-selection
    pub fn waste(&self, feerate: FeeRate, long_term_feerate: FeeRate) -> f32 {
        self.weight as f32 * feerate.spwu() + self.spend_weight as f32 * long_term_feerate.spwu()
    }
}

/// [`CoinSelector`] is responsible for selecting and deselecting from a set of canididates.
///
/// You can do this manually by calling methods like [`select`] or automatically with methods like [`branch_and_bound`].
///
/// [`select`]: CoinSelector::select
/// [`branch_and_bound`]: CoinSelector::branch_and_bound
#[derive(Debug, Clone)]
pub struct CoinSelector<'a> {
    base_weight: u32,
    candidates: &'a [WeightedValue],
    selected: Cow<'a, BTreeSet<usize>>,
    banned: Cow<'a, BTreeSet<usize>>,
    candidate_order: Cow<'a, Vec<usize>>,
}

/// A target value to select for along with feerate constraints.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// The minimum feerate that the selection must have
    pub feerate: FeeRate,
    /// The minimum fee the selection must have
    pub min_fee: u64,
    /// The minmum value that should be left for the output
    pub value: u64,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            feerate: FeeRate::default_min_relay_fee(),
            min_fee: 0, // TODO figure out what the actual network rule is for this
            value: 0,
        }
    }
}

impl<'a> CoinSelector<'a> {
    /// Creates a new coin selector from some candidate inputs and a `base_weight`.
    ///
    /// The `base_weight` is the weight of the transaction without any inputs and without a change
    /// output.
    ///
    /// Note that methods in `CoinSelector` will refer to inputs by the index in the `candidates`
    /// slice you pass in.
    // TODO: constructor should be number of outputs and output weight instead so we can keep track
    // of varint number of outputs
    pub fn new(candidates: &'a [WeightedValue], base_weight: u32) -> Self {
        Self {
            base_weight,
            candidates,
            selected: Cow::Owned(Default::default()),
            banned: Cow::Owned(Default::default()),
            candidate_order: Cow::Owned((0..candidates.len()).collect()),
        }
    }

    /// Iterate over all the candidates in their currently sorted order. Each item has the original
    /// index with the candidate.
    pub fn candidates(
        &self,
    ) -> impl DoubleEndedIterator<Item = (usize, WeightedValue)> + ExactSizeIterator + '_ {
        self.candidate_order
            .iter()
            .map(move |i| (*i, self.candidates[*i]))
    }

    /// Get the candidate at `index`. `index` refers to its position in the original `candidates` slice passed
    /// into [`CoinSelector::new`].
    pub fn candidate(&self, index: usize) -> WeightedValue {
        self.candidates[index]
    }

    /// Deselect a candidate at `index`. `index` refers to its position in the original `candidates` slice passed
    /// into [`CoinSelector::new`].
    pub fn deselect(&mut self, index: usize) -> bool {
        self.selected.to_mut().remove(&index)
    }

    /// Convienince method to pick elements of a slice by the indexes that are currently selected.
    /// Obviously the slice must represent the inputs ordered in the same way as when they were
    /// passed to `Candidates::new`.
    pub fn apply_selection<T>(&self, candidates: &'a [T]) -> impl Iterator<Item = &'a T> + '_ {
        self.selected.iter().map(move |i| &candidates[*i])
    }

    /// Select the input at `index`. `index` refers to its position in the original `candidates` slice passed
    /// into [`CoinSelector::new`].
    pub fn select(&mut self, index: usize) -> bool {
        assert!(index < self.candidates.len());
        self.selected.to_mut().insert(index)
    }

    /// Select the next unselected candidate in the sorted order fo the candidates.
    pub fn select_next(&mut self) -> bool {
        let next = self.unselected_indexes().next();
        if let Some(next) = next {
            self.select(next);
            true
        } else {
            false
        }
    }

    /// Ban an input from being selected. Banning the input means it won't show up in [`unselected`]
    /// or [`unselected_indexes`]. Note it can still be manually selected.
    ///
    /// `index` refers to its position in the original `candidates` slice passed into [`CoinSelector::new`].
    ///
    /// [`unselected`]: Self::unselected
    /// [`unselected_indexes`]: Self::unselected_indexes
    pub fn ban(&mut self, index: usize) {
        self.banned.to_mut().insert(index);
    }

    /// Gets the list of inputs that have been banned by [`ban`].
    ///
    /// [`ban`]: Self::ban
    pub fn banned(&self) -> &BTreeSet<usize> {
        &self.banned
    }

    /// Is the input at `index` selected. `index` refers to its position in the original
    /// `candidates` slice passed into [`CoinSelector::new`].
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    /// Is meeting this `target` possible with the current selection with this `drain` (i.e. change output).
    /// Note this will respect [`ban`]ned candidates.
    ///
    /// This simply selects all effective inputs at the target's feerate and checks whether we have
    /// enough value.
    ///
    /// [`ban`]: Self::ban
    pub fn is_selection_possible(&self, target: Target, drain: Drain) -> bool {
        let mut test = self.clone();
        test.select_all_effective(target.feerate);
        test.is_target_met(target, drain)
    }

    /// Is meeting the target *plausible* with this `change_policy`.
    /// Note this will respect [`ban`]ned candidates.
    ///
    /// This is very similar to [`is_selection_possible`] except that you pass in a change policy.
    /// This method will give the right answer as long as `change_policy` is monotone but otherwise
    /// can it can give false negatives.
    ///
    /// [`is_selection_possible`]: Self::is_selection_possible
    pub fn is_selection_plausible_with_change_policy(
        &self,
        target: Target,
        change_policy: &impl Fn(&CoinSelector<'a>, Target) -> Drain,
    ) -> bool {
        let mut test = self.clone();
        test.select_all_effective(target.feerate);
        test.is_target_met(target, change_policy(&test, target))
    }

    /// Returns true if no candidates have been selected.
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

    pub fn input_weight(&self) -> u32 {
        let witness_header_extra_weight = self
            .selected()
            .find(|(_, wv)| wv.is_segwit)
            .map(|_| 2)
            .unwrap_or(0);
        let vin_count_varint_extra_weight = {
            let input_count = self.selected().map(|(_, wv)| wv.input_count).sum::<usize>();
            (varint_size(input_count) - 1) * 4
        };

        self.selected_weight() + witness_header_extra_weight + vin_count_varint_extra_weight
    }

    /// Absolute value sum of all selected inputs.
    pub fn selected_value(&self) -> u64 {
        self.selected
            .iter()
            .map(|&index| self.candidates[index].value)
            .sum()
    }

    /// Current weight of template tx + selected inputs.
    pub fn weight(&self, drain_weight: u32) -> u32 {
        // TODO take into account whether drain tips over varint for number of outputs
        //
        // TODO: take into account the witness stack length for each input
        self.base_weight + self.input_weight() + drain_weight
    }

    /// How much the current selection overshoots the value needed to acheive `target`.
    ///
    /// In order for the resulting transaction to be valid this must be 0.
    pub fn excess(&self, target: Target, drain: Drain) -> i64 {
        self.selected_value() as i64
            - target.value as i64
            - drain.value as i64
            - self.implied_fee(target.feerate, target.min_fee, drain.weight) as i64
    }

    pub fn rate_excess(&self, target: Target, drain: Drain) -> i64 {
        self.selected_value() as i64
            - target.value as i64
            - drain.value as i64
            - self.implied_fee_from_feerate(target.feerate, drain.weight) as i64
    }

    pub fn absolute_excess(&self, target: Target, drain: Drain) -> i64 {
        self.selected_value() as i64
            - target.value as i64
            - drain.value as i64
            - target.min_fee as i64
    }

    /// The feerate the transaction would have if we were to use this selection of inputs to acheive
    /// the
    pub fn implied_feerate(&self, target_value: u64, drain: Drain) -> FeeRate {
        let numerator = self.selected_value() as i64 - target_value as i64 - drain.value as i64;
        let denom = self.weight(drain.weight);
        FeeRate::from_sat_per_wu(numerator as f32 / denom as f32)
    }

    pub fn implied_fee(&self, feerate: FeeRate, min_fee: u64, drain_weight: u32) -> u64 {
        (self.implied_fee_from_feerate(feerate, drain_weight)).max(min_fee)
    }

    pub fn implied_fee_from_feerate(&self, feerate: FeeRate, drain_weight: u32) -> u64 {
        (self.weight(drain_weight) as f32 * feerate.spwu()).ceil() as u64
    }

    /// The value of the current selected inputs minus the fee needed to pay for the selected inputs
    pub fn effective_value(&self, feerate: FeeRate) -> i64 {
        self.selected_value() as i64 - (self.input_weight() as f32 * feerate.spwu()).ceil() as i64
    }

    // /// Waste sum of all selected inputs.
    fn selected_waste(&self, feerate: FeeRate, long_term_feerate: FeeRate) -> f32 {
        self.selected_weight() as f32 * (feerate.spwu() - long_term_feerate.spwu())
    }

    pub fn sort_candidates_by<F>(&mut self, mut cmp: F)
    where
        F: FnMut((usize, WeightedValue), (usize, WeightedValue)) -> core::cmp::Ordering,
    {
        let order = self.candidate_order.to_mut();
        let candidates = &self.candidates;
        order.sort_by(|a, b| cmp((*a, candidates[*a]), (*b, candidates[*b])))
    }

    pub fn sort_candidates_by_key<F, K>(&mut self, mut key_fn: F)
    where
        F: FnMut((usize, WeightedValue)) -> K,
        K: Ord,
    {
        self.sort_candidates_by(|a, b| key_fn(a).cmp(&key_fn(b)))
    }

    pub fn sort_candidates_by_descending_value_pwu(&mut self) {
        self.sort_candidates_by_key(|(_, wv)| core::cmp::Reverse(wv.value_pwu()));
    }

    pub fn waste(
        &self,
        target: Target,
        long_term_feerate: FeeRate,
        drain: Drain,
        excess_discount: f32,
    ) -> f32 {
        debug_assert!(excess_discount >= 0.0 && excess_discount <= 1.0);
        let mut waste = self.selected_waste(target.feerate, long_term_feerate);

        if drain.is_none() {
            // We don't allow negative excess waste since negative excess just means you haven't
            // satisified target yet in which case you probably shouldn't be calling this function.
            let mut excess_waste = self.excess(target, drain).max(0) as f32;
            // we allow caller to discount this waste depending on how wasteful excess actually is
            // to them.
            excess_waste *= excess_discount.max(0.0).min(1.0);
            waste += excess_waste;
        } else {
            waste += drain.weight as f32 * target.feerate.spwu()
                + drain.spend_weight as f32 * long_term_feerate.spwu();
        }

        waste
    }

    pub fn selected(&self) -> impl ExactSizeIterator<Item = (usize, WeightedValue)> + '_ {
        self.selected
            .iter()
            .map(move |&index| (index, self.candidates[index]))
    }

    pub fn unselected(&self) -> impl DoubleEndedIterator<Item = (usize, WeightedValue)> + '_ {
        self.unselected_indexes()
            .map(move |i| (i, self.candidates[i]))
    }

    pub fn selected_indexes(&self) -> &BTreeSet<usize> {
        &self.selected
    }

    pub fn unselected_indexes(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.candidate_order
            .iter()
            .filter(move |index| !(self.selected.contains(index) || self.banned.contains(index)))
            .map(|index| *index)
    }

    pub fn is_exhausted(&self) -> bool {
        self.unselected_indexes().next().is_none()
    }

    pub fn is_target_met(&self, target: Target, drain: Drain) -> bool {
        self.excess(target, drain) >= 0
    }

    pub fn select_all(&mut self) {
        loop {
            if !self.select_next() {
                break;
            }
        }
    }

    pub fn select_all_effective(&mut self, feerate: FeeRate) {
        // TODO: do this without allocating
        for i in self.unselected_indexes().collect::<Vec<_>>() {
            if self.candidates[i].effective_value(feerate) > Ordf32(0.0) {
                self.select(i);
            }
        }
    }

    #[must_use]
    pub fn select_until_target_met(&mut self, target: Target, drain: Drain) -> Option<()> {
        self.select_until(|cs| cs.is_target_met(target, drain))
    }

    #[must_use]
    pub fn select_until(
        &mut self,
        mut predicate: impl FnMut(&CoinSelector<'a>) -> bool,
    ) -> Option<()> {
        loop {
            if predicate(&*self) {
                break Some(());
            }

            if !self.select_next() {
                break None;
            }
        }
    }

    pub fn select_iter(self) -> SelectIter<'a> {
        SelectIter { cs: self.clone() }
    }

    pub fn branch_and_bound<M: BnBMetric>(
        &self,
        metric: M,
    ) -> impl Iterator<Item = Option<(CoinSelector<'a>, M::Score)>> {
        crate::bnb::BnbIter::new(self.clone(), metric)
    }
}

pub struct SelectIter<'a> {
    cs: CoinSelector<'a>,
}

impl<'a> Iterator for SelectIter<'a> {
    type Item = (CoinSelector<'a>, usize, WeightedValue);

    fn next(&mut self) -> Option<Self::Item> {
        let (index, wv) = self.cs.unselected().next()?;
        self.cs.select(index);
        Some((self.cs.clone(), index, wv))
    }
}

impl<'a> DoubleEndedIterator for SelectIter<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let (index, wv) = self.cs.unselected().next_back()?;
        self.cs.select(index);
        Some((self.cs.clone(), index, wv))
    }
}

impl<'a> core::fmt::Display for CoinSelector<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[")?;
        let mut candidates = self.candidates().peekable();

        while let Some((i, _)) = candidates.next() {
            write!(f, "{}", i)?;
            if self.is_selected(i) {
                write!(f, "✔")?;
            } else if self.banned().contains(&i) {
                write!(f, "✘")?
            } else {
                write!(f, "☐")?;
            }

            if candidates.peek().is_some() {
                write!(f, ", ")?;
            }
        }

        write!(f, "]")
    }
}
