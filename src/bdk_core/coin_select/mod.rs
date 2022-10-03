mod bnb;
pub use bnb::*;

mod coin_selector;
pub use coin_selector::*;

/// Txin "base" fields include `outpoint` (32+4) and `nSequence` (4). This does not include
/// `scriptSigLen` or `scriptSig`.
pub const TXIN_BASE_WEIGHT: u32 = (32 + 4 + 4) * 4;

/// Helper to calculate varint size. `v` is the value the varint represents.
pub fn varint_size(v: usize) -> u32 {
    if v <= 0xfc {
        return 1;
    }
    if v <= 0xffff {
        return 3;
    }
    if v <= 0xffff_ffff {
        return 5;
    }

    9
}

pub mod evaluate_cs {
    //! Coin Select Evaluation
    //!
    //! Module to evaluate coin selection.

    use super::{CoinSelector, ExcessStrategyKind, Selection};

    /// Evaluates a coin selection algorithm.
    pub fn evaluate<F>(
        initial_selector: CoinSelector,
        mut select: F,
    ) -> Result<Evaluation, EvaluationFailure>
    where
        F: FnMut(&mut CoinSelector) -> bool,
    {
        let mut selector = initial_selector.clone();
        let start_time = std::time::SystemTime::now();
        let has_solution = select(&mut selector);
        let elapsed = start_time.elapsed().expect("system time error");

        if has_solution {
            let solution = selector.finish().expect("failed to finish what we started");

            let elapsed_per_candidate = elapsed / selector.candidates.len() as _;

            let waste_vec = solution
                .excess_strategies
                .iter()
                .map(|(_, s)| s.waste)
                .collect::<Vec<_>>();

            let waste_mean = waste_vec.iter().sum::<i64>() as f32 / waste_vec.len() as f32;
            let waste_median = if waste_vec.len() % 2 != 0 {
                waste_vec[waste_vec.len() / 2] as f32
            } else {
                (waste_vec[(waste_vec.len() - 1) / 2] + waste_vec[waste_vec.len() / 2]) as f32 / 2.0
            };

            Ok(Evaluation {
                initial_selector,
                solution,
                elapsed,
                elapsed_per_candidate,
                waste_median,
                waste_mean,
            })
        } else {
            Err(EvaluationFailure {
                initial: initial_selector,
                elapsed,
            })
        }
    }

    /// Evaluation result of a coin selection.
    #[derive(Debug, Clone)]
    pub struct Evaluation<'a> {
        /// Initial [`CoinSelector`].
        pub initial_selector: CoinSelector<'a>,
        /// Final solution.
        pub solution: Selection,

        /// Elapsed duration of coin selection.
        pub elapsed: std::time::Duration,
        /// Elapsed duration per candidate.
        pub elapsed_per_candidate: std::time::Duration,

        /// Median waste.
        pub waste_median: f32,
        /// Mean waste.
        pub waste_mean: f32,
    }

    impl<'a> Evaluation<'a> {
        /// Obtain waste of specified excess strategy kind.
        pub fn waste(&self, strategy_kind: ExcessStrategyKind) -> i64 {
            self.solution.excess_strategies[&strategy_kind].waste
        }

        /// Obtain feerate offset of specified excess strategy kind.
        pub fn feerate_offset(&self, strategy_kind: ExcessStrategyKind) -> f32 {
            let target_rate = self.initial_selector.opts.target_feerate;
            let actual_rate = self.solution.excess_strategies[&strategy_kind].feerate();
            actual_rate - target_rate
        }
    }

    impl<'a> core::fmt::Display for Evaluation<'a> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            writeln!(f, "Evaluation:")?;
            writeln!(
                f,
                "\t* Candidates: {}",
                self.initial_selector.candidates.len()
            )?;
            writeln!(
                f,
                "\t* Initial selection: {}",
                self.initial_selector.selected_count()
            )?;
            writeln!(f, "\t* Final selection: {}", self.solution.selected.len())?;
            writeln!(f, "\t* Elapsed: {:?}", self.elapsed)?;
            writeln!(
                f,
                "\t* Elapsed per candidate: {:?}",
                self.elapsed_per_candidate
            )?;
            Ok(())
        }
    }

    /// Evaluation failure.
    #[derive(Debug, Clone)]
    pub struct EvaluationFailure<'a> {
        initial: CoinSelector<'a>,
        elapsed: std::time::Duration,
    }

    impl<'a> core::fmt::Display for EvaluationFailure<'a> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(
                f,
                "cs algorithm failed to find a solution: elapsed={}s target_feerate={}sats/wu",
                self.elapsed.as_secs(),
                self.initial.opts.target_feerate
            )
        }
    }

    impl<'a> std::error::Error for EvaluationFailure<'a> {}
}

#[cfg(test)]
pub mod tester {
    use super::*;
    use bitcoin::{
        secp256k1::{All, Secp256k1},
        TxOut,
    };
    use miniscript::{
        // plan::{Assets, Plan},
        Descriptor,
        DescriptorPublicKey,
        DescriptorTrait,
    };

    #[derive(Debug, Clone)]
    pub struct TestCandidate {
        pub txo: TxOut,
        // pub plan: Plan<DescriptorPublicKey>,
    }

    impl From<TestCandidate> for WeightedValue {
        fn from(test_candidate: TestCandidate) -> Self {
            Self {
                value: test_candidate.txo.value,
                // weight: TXIN_BASE_WEIGHT + test_candidate.plan.expected_weight() as u32,
                weight: TXIN_BASE_WEIGHT + 65, // TODO: Is this correct for schnorr?
                input_count: 1,
                // is_segwit: test_candidate.plan.witness_version().is_some(),
                is_segwit: true,
            }
        }
    }

    pub struct Tester {
        descriptor: Descriptor<DescriptorPublicKey>,
        // assets: Assets<DescriptorPublicKey>,
    }

    impl Tester {
        pub fn new(secp: &Secp256k1<All>, desc_str: &str) -> Self {
            // let desc_str = "tr(xprv9uBuvtdjghkz8D1qzsSXS9Vs64mqrUnXqzNccj2xcvnCHPpXKYE1U2Gbh9CDHk8UPyF2VuXpVkDA7fk5ZP4Hd9KnhUmTscKmhee9Dp5sBMK)";
            let (descriptor, _seckeys) =
                Descriptor::<DescriptorPublicKey>::parse_descriptor(secp, desc_str).unwrap();

            // let assets = Assets {
            //     keys: seckeys.keys().cloned().collect(),
            //     ..Default::default()
            // };

            // Self { descriptor, assets }
            Self { descriptor }
        }

        pub fn gen_candidate(&self, derivation_index: u32, value: u64) -> TestCandidate {
            let secp = Secp256k1::new();
            // let descriptor = self.descriptor.at_derivation_index(derivation_index);
            let descriptor = self
                .descriptor
                .derived_descriptor(&secp, derivation_index)
                .expect("woops");
            // let plan = descriptor.plan_satisfaction(&self.assets).unwrap();
            let txo = TxOut {
                value,
                script_pubkey: descriptor.script_pubkey(),
            };
            // TestCandidate { txo, plan }
            TestCandidate { txo }
        }

        pub fn gen_weighted_value(&self, value: u64) -> WeightedValue {
            self.gen_candidate(0, value).into()
        }

        pub fn gen_weighted_values(&self, out: &mut Vec<WeightedValue>, count: usize, value: u64) {
            (0..count).for_each(|_| out.push(self.gen_candidate(0, value).into()))
        }

        pub fn gen_opts(&self, recipient_value: u64) -> CoinSelectorOpt {
            let recipient = self.gen_candidate(0, recipient_value);
            let drain = self.gen_candidate(0, 0);
            CoinSelectorOpt::fund_outputs(
                &[recipient.txo],
                &drain.txo,
                65,
                // drain.plan.expected_weight() as u32,
            )
        }
    }
}
