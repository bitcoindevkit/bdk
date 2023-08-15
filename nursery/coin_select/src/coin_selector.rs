use super::*;
#[allow(unused)] // some bug in <= 1.48.0 sees this as unused when it isn't
use crate::float::FloatExt;
use crate::{bnb::BnbMetric, float::Ordf32, FeeRate};
use alloc::{borrow::Cow, collections::BTreeSet, vec::Vec};

/// [`CoinSelector`] is responsible for selecting and deselecting from a set of canididates.
///
/// You can do this manually by calling methods like [`select`] or automatically with methods like [`bnb_solutions`].
///
/// [`select`]: CoinSelector::select
/// [`bnb_solutions`]: CoinSelector::bnb_solutions
#[derive(Debug, Clone)]
pub struct CoinSelector<'a> {
    base_weight: u32,
    candidates: &'a [Candidate],
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
    pub fn new(candidates: &'a [Candidate], base_weight: u32) -> Self {
        Self {
            base_weight,
            candidates,
            selected: Cow::Owned(Default::default()),
            banned: Cow::Owned(Default::default()),
            candidate_order: Cow::Owned((0..candidates.len()).collect()),
        }
    }

    /// Creates a new coin selector from some candidate inputs and a list of `output_weights`.
    ///
    /// This is a convenience method to calculate the `base_weight` from a set of recipient output
    /// weights. This is equivalent to calculating the `base_weight` yourself and calling
    /// [`CoinSelector::new`].
    pub fn fund_outputs(
        candidates: &'a [Candidate],
        output_weights: impl Iterator<Item = u32>,
    ) -> Self {
        let (output_count, output_weight_total) =
            output_weights.fold((0_usize, 0_u32), |(n, w), a| (n + 1, w + a));

        let base_weight = (4 /* nVersion */
            + 4 /* nLockTime */
            + varint_size(0) /* inputs varint */
            + varint_size(output_count)/* outputs varint */)
            * 4
            + output_weight_total;
        Self::new(candidates, base_weight)
    }

    /// Iterate over all the candidates in their currently sorted order. Each item has the original
    /// index with the candidate.
    pub fn candidates(
        &self,
    ) -> impl DoubleEndedIterator<Item = (usize, Candidate)> + ExactSizeIterator + '_ {
        self.candidate_order
            .iter()
            .map(move |i| (*i, self.candidates[*i]))
    }

    /// Get the candidate at `index`. `index` refers to its position in the original `candidates` slice passed
    /// into [`CoinSelector::new`].
    pub fn candidate(&self, index: usize) -> Candidate {
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
        let next = self.unselected_indices().next();
        if let Some(next) = next {
            self.select(next);
            true
        } else {
            false
        }
    }

    /// Ban an input from being selected. Banning the input means it won't show up in [`unselected`]
    /// or [`unselected_indices`]. Note it can still be manually selected.
    ///
    /// `index` refers to its position in the original `candidates` slice passed into [`CoinSelector::new`].
    ///
    /// [`unselected`]: Self::unselected
    /// [`unselected_indices`]: Self::unselected_indices
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
    /// [`ban`]: Self::ban
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

    /// The weight of the inputs including the witness header and the varint for the number of
    /// inputs.
    pub fn input_weight(&self) -> u32 {
        let is_segwit_tx = self.selected().any(|(_, wv)| wv.is_segwit);
        let witness_header_extra_weight = is_segwit_tx as u32 * 2;
        let vin_count_varint_extra_weight = {
            let input_count = self.selected().map(|(_, wv)| wv.input_count).sum::<usize>();
            (varint_size(input_count) - 1) * 4
        };

        let selected_weight: u32 = self
            .selected()
            .map(|(_, candidate)| {
                let mut weight = candidate.weight;
                if is_segwit_tx && !candidate.is_segwit {
                    // non-segwit candidates do not have the witness length field included in their
                    // weight field so we need to add 1 here if it's in a segwit tx.
                    weight += 1;
                }
                weight
            })
            .sum();

        selected_weight + witness_header_extra_weight + vin_count_varint_extra_weight
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
        self.base_weight + self.input_weight() + drain_weight
    }

    /// How much the current selection overshoots the value needed to achieve `target`.
    ///
    /// In order for the resulting transaction to be valid this must be 0.
    pub fn excess(&self, target: Target, drain: Drain) -> i64 {
        self.selected_value() as i64
            - target.value as i64
            - drain.value as i64
            - self.implied_fee(target.feerate, target.min_fee, drain.weights.output_weight) as i64
    }

    /// How much the current selection overshoots the value need to satisfy `target.feerate` and
    /// `target.value` (while ignoring `target.min_fee`).
    pub fn rate_excess(&self, target: Target, drain: Drain) -> i64 {
        self.selected_value() as i64
            - target.value as i64
            - drain.value as i64
            - self.implied_fee_from_feerate(target.feerate, drain.weights.output_weight) as i64
    }

    /// How much the current selection overshoots the value needed to satisfy `target.min_fee` and
    /// `target.value` (while ignoring `target.feerate`).
    pub fn absolute_excess(&self, target: Target, drain: Drain) -> i64 {
        self.selected_value() as i64
            - target.value as i64
            - drain.value as i64
            - target.min_fee as i64
    }

    /// The feerate the transaction would have if we were to use this selection of inputs to achieve
    /// the `target_value`.
    pub fn implied_feerate(&self, target_value: u64, drain: Drain) -> FeeRate {
        let numerator = self.selected_value() as i64 - target_value as i64 - drain.value as i64;
        let denom = self.weight(drain.weights.output_weight);
        FeeRate::from_sat_per_wu(numerator as f32 / denom as f32)
    }

    /// The fee the current selection should pay to reach `feerate` and provide `min_fee`
    fn implied_fee(&self, feerate: FeeRate, min_fee: u64, drain_weight: u32) -> u64 {
        (self.implied_fee_from_feerate(feerate, drain_weight)).max(min_fee)
    }

    fn implied_fee_from_feerate(&self, feerate: FeeRate, drain_weight: u32) -> u64 {
        (self.weight(drain_weight) as f32 * feerate.spwu()).ceil() as u64
    }

    /// The value of the current selected inputs minus the fee needed to pay for the selected inputs
    pub fn effective_value(&self, feerate: FeeRate) -> i64 {
        self.selected_value() as i64 - (self.input_weight() as f32 * feerate.spwu()).ceil() as i64
    }

    // /// Waste sum of all selected inputs.
    fn input_waste(&self, feerate: FeeRate, long_term_feerate: FeeRate) -> f32 {
        self.input_weight() as f32 * (feerate.spwu() - long_term_feerate.spwu())
    }

    /// Sorts the candidates by the comparision function.
    ///
    /// The comparision function takes the candidates's index and the [`Candidate`].
    ///
    /// Note this function does not change the index of the candidates after sorting, just the order
    /// in which they will be returned when interating over them in [`candidates`] and [`unselected`].
    ///
    /// [`candidates`]: CoinSelector::candidates
    /// [`unselected`]: CoinSelector::unselected
    pub fn sort_candidates_by<F>(&mut self, mut cmp: F)
    where
        F: FnMut((usize, Candidate), (usize, Candidate)) -> core::cmp::Ordering,
    {
        let order = self.candidate_order.to_mut();
        let candidates = &self.candidates;
        order.sort_by(|a, b| cmp((*a, candidates[*a]), (*b, candidates[*b])))
    }

    /// Sorts the candidates by the key function.
    ///
    /// The key function takes the candidates's index and the [`Candidate`].
    ///
    /// Note this function does not change the index of the candidates after sorting, just the order
    /// in which they will be returned when interating over them in [`candidates`] and [`unselected`].
    ///
    /// [`candidates`]: CoinSelector::candidates
    /// [`unselected`]: CoinSelector::unselected
    pub fn sort_candidates_by_key<F, K>(&mut self, mut key_fn: F)
    where
        F: FnMut((usize, Candidate)) -> K,
        K: Ord,
    {
        self.sort_candidates_by(|a, b| key_fn(a).cmp(&key_fn(b)))
    }

    /// Sorts the candidates by descending value per weight unit
    pub fn sort_candidates_by_descending_value_pwu(&mut self) {
        self.sort_candidates_by_key(|(_, wv)| core::cmp::Reverse(wv.value_pwu()));
    }

    /// The waste created by the current selection as measured by the [waste metric].
    ///
    /// You can pass in an `excess_discount` which must be between `0.0..1.0`. Passing in `1.0` gives you no discount
    ///
    /// [waste metric]: https://bitcoin.stackexchange.com/questions/113622/what-does-waste-metric-mean-in-the-context-of-coin-selection
    pub fn waste(
        &self,
        target: Target,
        long_term_feerate: FeeRate,
        drain: Drain,
        excess_discount: f32,
    ) -> f32 {
        debug_assert!((0.0..=1.0).contains(&excess_discount));
        let mut waste = self.input_waste(target.feerate, long_term_feerate);

        if drain.is_none() {
            // We don't allow negative excess waste since negative excess just means you haven't
            // satisified target yet in which case you probably shouldn't be calling this function.
            let mut excess_waste = self.excess(target, drain).max(0) as f32;
            // we allow caller to discount this waste depending on how wasteful excess actually is
            // to them.
            excess_waste *= excess_discount.max(0.0).min(1.0);
            waste += excess_waste;
        } else {
            waste += drain.weights.output_weight as f32 * target.feerate.spwu()
                + drain.weights.spend_weight as f32 * long_term_feerate.spwu();
        }

        waste
    }

    /// The selected candidates with their index.
    pub fn selected(&self) -> impl ExactSizeIterator<Item = (usize, Candidate)> + '_ {
        self.selected
            .iter()
            .map(move |&index| (index, self.candidates[index]))
    }

    /// The unselected candidates with their index.
    ///
    /// The candidates are returned in sorted order. See [`sort_candidates_by`].
    ///
    /// [`sort_candidates_by`]: Self::sort_candidates_by
    pub fn unselected(&self) -> impl DoubleEndedIterator<Item = (usize, Candidate)> + '_ {
        self.unselected_indices()
            .map(move |i| (i, self.candidates[i]))
    }

    /// The indices of the selelcted candidates.
    pub fn selected_indices(&self) -> &BTreeSet<usize> {
        &self.selected
    }

    /// The indices of the unselected candidates.
    ///
    /// This excludes candidates that have been selected or [`banned`].
    ///
    /// [`banned`]: Self::ban
    pub fn unselected_indices(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.candidate_order
            .iter()
            .filter(move |index| !(self.selected.contains(index) || self.banned.contains(index)))
            .copied()
    }

    /// Whether there are any unselected candidates left.
    pub fn is_exhausted(&self) -> bool {
        self.unselected_indices().next().is_none()
    }

    /// Whether the constraints of `Target` have been met if we include the `drain` ouput.
    pub fn is_target_met(&self, target: Target, drain: Drain) -> bool {
        self.excess(target, drain) >= 0
    }

    /// Select all unselected candidates
    pub fn select_all(&mut self) {
        loop {
            if !self.select_next() {
                break;
            }
        }
    }

    /// Select all candidates with an *effective value* greater than 0 at the provided `feerate`.
    ///
    /// A candidate if effective if it provides more value than it takes to pay for at `feerate`.
    pub fn select_all_effective(&mut self, feerate: FeeRate) {
        // TODO: do this without allocating
        for i in self.unselected_indices().collect::<Vec<_>>() {
            if self.candidates[i].effective_value(feerate) > Ordf32(0.0) {
                self.select(i);
            }
        }
    }

    /// Select candidates until `target` has been met assuming the `drain` output is attached.
    ///
    /// Returns an `Some(_)` if it was able to meet the target.
    pub fn select_until_target_met(
        &mut self,
        target: Target,
        drain: Drain,
    ) -> Result<(), InsufficientFunds> {
        self.select_until(|cs| cs.is_target_met(target, drain))
            .ok_or_else(|| InsufficientFunds {
                missing: self.excess(target, drain).unsigned_abs(),
            })
    }

    /// Select candidates until some predicate has been satisfied.
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

    /// Return an iterator that can be used to select candidates.
    pub fn select_iter(self) -> SelectIter<'a> {
        SelectIter { cs: self.clone() }
    }

    /// Returns a branch and bound iterator, given a `metric`.
    ///
    /// Not every iteration will return a solution. If a solution is found, we return the selection
    /// and score. Each subsequent solution of the iterator guarantees a higher score than the last.
    ///
    /// Most of the time, you would want to use [`CoinSelector::run_bnb`] instead.
    pub fn bnb_solutions<M: BnbMetric>(
        &self,
        metric: M,
    ) -> impl Iterator<Item = Option<(CoinSelector<'a>, M::Score)>> {
        crate::bnb::BnbIter::new(self.clone(), metric)
    }

    /// Run branch and bound until we cannot find a better solution, or we reach `max_rounds`.
    ///
    /// If a solution is found, the [`BnBMetric::Score`] is returned. Otherwise, we error with
    /// [`NoBnbSolution`].
    ///
    /// To access to raw bnb iterator, use [`CoinSelector::bnb_solutions`].
    pub fn run_bnb<M: BnbMetric>(
        &mut self,
        metric: M,
        max_rounds: usize,
    ) -> Result<M::Score, NoBnbSolution> {
        let mut rounds = 0_usize;
        let (selector, score) = self
            .bnb_solutions(metric)
            .inspect(|_| rounds += 1)
            .take(max_rounds)
            .flatten()
            .last()
            .ok_or(NoBnbSolution { max_rounds, rounds })?;
        *self = selector;
        Ok(score)
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

/// A `Candidate` represents an input candidate for [`CoinSelector`]. This can either be a
/// single UTXO, or a group of UTXOs that should be spent together.
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    /// Total value of the UTXO(s) that this [`Candidate`] represents.
    pub value: u64,
    /// Total weight of including this/these UTXO(s).
    /// `txin` fields: `prevout`, `nSequence`, `scriptSigLen`, `scriptSig`, `scriptWitnessLen`,
    /// `scriptWitness` should all be included.
    pub weight: u32,
    /// Total number of inputs; so we can calculate extra `varint` weight due to `vin` len changes.
    pub input_count: usize,
    /// Whether this [`Candidate`] contains at least one segwit spend.
    pub is_segwit: bool,
}

impl Candidate {
    pub fn new_tr_keyspend(value: u64) -> Self {
        let weight = TXIN_BASE_WEIGHT + TR_KEYSPEND_SATISFACTION_WEIGHT;
        Self::new(value, weight, true)
    }
    /// Create a new [`Candidate`] that represents a single input.
    ///
    /// `satisfaction_weight` is the weight of `scriptSigLen + scriptSig + scriptWitnessLen +
    /// scriptWitness`.
    pub fn new(value: u64, satisfaction_weight: u32, is_segwit: bool) -> Candidate {
        let weight = TXIN_BASE_WEIGHT + satisfaction_weight;
        Candidate {
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

/// A structure that represents the weight costs of a drain (a.k.a. change) output.
///
/// This structure can also represent multiple outputs.
#[derive(Default, Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct DrainWeights {
    /// The weight of including this drain output.
    ///
    /// This must take into account the weight change from varint output count.
    pub output_weight: u32,
    /// The weight of spending this drain output (in the future).
    pub spend_weight: u32,
}

impl DrainWeights {
    /// The waste of adding this drain to a transaction according to the [waste metric].
    ///
    /// [waste metric]: https://bitcoin.stackexchange.com/questions/113622/what-does-waste-metric-mean-in-the-context-of-coin-selection
    pub fn waste(&self, feerate: FeeRate, long_term_feerate: FeeRate) -> f32 {
        self.output_weight as f32 * feerate.spwu()
            + self.spend_weight as f32 * long_term_feerate.spwu()
    }

    pub fn new_tr_keyspend() -> Self {
        Self {
            output_weight: TXOUT_BASE_WEIGHT + TR_SPK_WEIGHT,
            spend_weight: TXIN_BASE_WEIGHT + TR_KEYSPEND_SATISFACTION_WEIGHT,
        }
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
    /// Weight of adding drain output and spending the drain output.
    pub weights: DrainWeights,
    /// The value that should be assigned to the drain.
    pub value: u64,
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
    /// [waste metric]: https://bitcoin.stackexchange.com/questions/113622/what-does-waste-metric-mean-in-the-context-of-coin-selection
    pub fn waste(&self, feerate: FeeRate, long_term_feerate: FeeRate) -> f32 {
        self.weights.waste(feerate, long_term_feerate)
    }
}

/// The `SelectIter` allows you to select candidates by calling [`Iterator::next`].
///
/// The [`Iterator::Item`] is a tuple of `(selector, last_selected_index, last_selected_candidate)`.
pub struct SelectIter<'a> {
    cs: CoinSelector<'a>,
}

impl<'a> Iterator for SelectIter<'a> {
    type Item = (CoinSelector<'a>, usize, Candidate);

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

#[derive(Clone, Debug, Copy, PartialEq, Eq)]
pub struct InsufficientFunds {
    missing: u64,
}

impl core::fmt::Display for InsufficientFunds {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "Insufficient funds. Missing {} sats.", self.missing)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InsufficientFunds {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoBnbSolution {
    max_rounds: usize,
    rounds: usize,
}

impl core::fmt::Display for NoBnbSolution {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "No bnb solution found after {} rounds (max rounds is {}).",
            self.rounds, self.max_rounds
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for NoBnbSolution {}
