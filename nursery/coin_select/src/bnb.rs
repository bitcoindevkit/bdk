use core::cmp::Reverse;

use super::CoinSelector;
use alloc::collections::BinaryHeap;

#[derive(Debug)]
pub(crate) struct BnbIter<'a, M: BnbMetric> {
    queue: BinaryHeap<Branch<'a, M::Score>>,
    best: Option<M::Score>,
    /// The `BnBMetric` that will score each selection
    metric: M,
}

impl<'a, M: BnbMetric> Iterator for BnbIter<'a, M> {
    type Item = Option<(CoinSelector<'a>, M::Score)>;

    fn next(&mut self) -> Option<Self::Item> {
        // {
        //     println!("=========================== {:?}", self.best);
        //     for thing in self.queue.iter() {
        //         println!("{} {:?}", &thing.selector, thing.lower_bound);
        //     }
        //     let _ = std::io::stdin().read_line(&mut alloc::string::String::new());
        // }

        let branch = self.queue.pop()?;
        if let Some(best) = &self.best {
            // If the next thing in queue is not better than our best we're done.
            if *best < branch.lower_bound {
                // println!(
                //     "\t\t(SKIP) branch={} inclusion={} lb={:?}, score={:?}",
                //     branch.selector,
                //     !branch.is_exclusion,
                //     branch.lower_bound,
                //     self.metric.score(&branch.selector),
                // );
                return None;
            }
        }
        // println!(
        //     "\t\t( POP) branch={} inclusion={} lb={:?}, score={:?}",
        //     branch.selector,
        //     !branch.is_exclusion,
        //     branch.lower_bound,
        //     self.metric.score(&branch.selector),
        // );

        let selector = branch.selector;

        self.insert_new_branches(&selector);

        if branch.is_exclusion {
            return Some(None);
        }

        let score = match self.metric.score(&selector) {
            Some(score) => score,
            None => return Some(None),
        };

        if let Some(best_score) = &self.best {
            if score >= *best_score {
                return Some(None);
            }
        }
        self.best = Some(score.clone());
        Some(Some((selector, score)))
    }
}

impl<'a, M: BnbMetric> BnbIter<'a, M> {
    pub fn new(mut selector: CoinSelector<'a>, metric: M) -> Self {
        let mut iter = BnbIter {
            queue: BinaryHeap::default(),
            best: None,
            metric,
        };

        if iter.metric.requires_ordering_by_descending_value_pwu() {
            selector.sort_candidates_by_descending_value_pwu();
        }

        iter.consider_adding_to_queue(&selector, false);

        iter
    }

    fn consider_adding_to_queue(&mut self, cs: &CoinSelector<'a>, is_exclusion: bool) {
        let bound = self.metric.bound(cs);
        if let Some(bound) = bound {
            if self.best.is_none() || self.best.as_ref().unwrap() > &bound {
                let branch = Branch {
                    lower_bound: bound,
                    selector: cs.clone(),
                    is_exclusion,
                };
                // println!(
                //     "\t\t(PUSH) branch={} inclusion={} lb={:?}, score={:?}",
                //     branch.selector,
                //     !branch.is_exclusion,
                //     branch.lower_bound,
                //     self.metric.score(&branch.selector),
                // );
                self.queue.push(branch);
            }
        }
    }

    fn insert_new_branches(&mut self, cs: &CoinSelector<'a>) {
        let (next_index, next) = match cs.unselected().next() {
            Some(c) => c,
            None => return, // exhausted
        };

        // for the exclusion branch, we keep banning if candidates have the same weight and value
        let mut exclusion_cs = cs.clone();
        let to_ban = (next.value, next.weight);
        for (next_index, next) in cs.unselected() {
            if (next.value, next.weight) != to_ban {
                break;
            }
            exclusion_cs.ban(next_index);
        }
        self.consider_adding_to_queue(&exclusion_cs, true);

        let mut inclusion_cs = cs.clone();
        inclusion_cs.select(next_index);
        self.consider_adding_to_queue(&inclusion_cs, false);
    }
}

#[derive(Debug, Clone)]
struct Branch<'a, O> {
    lower_bound: O,
    selector: CoinSelector<'a>,
    is_exclusion: bool,
}

impl<'a, O: Ord> Ord for Branch<'a, O> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // NOTE: Reverse comparision `lower_bound` because we want a min-heap (by default BinaryHeap
        // is a max-heap).
        // NOTE: We tiebreak equal scores based on whether it's exlusion or not (preferring
        // inclusion). We do this because we want to try and get to evaluating complete selection
        // returning actual scores as soon as possible.
        core::cmp::Ord::cmp(
            &(Reverse(&self.lower_bound), !self.is_exclusion),
            &(Reverse(&other.lower_bound), !other.is_exclusion),
        )
    }
}

impl<'a, O: Ord> PartialOrd for Branch<'a, O> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a, O: PartialEq> PartialEq for Branch<'a, O> {
    fn eq(&self, other: &Self) -> bool {
        self.lower_bound == other.lower_bound
    }
}

impl<'a, O: PartialEq> Eq for Branch<'a, O> {}

/// A branch and bound metric.
pub trait BnbMetric {
    type Score: Ord + Clone + core::fmt::Debug;

    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score>;

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score>;

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        false
    }
}
