# BDK Coin Selection

`bdk_coin_select` is a tool to help you select inputs for making Bitcoin (ticker: BTC) transactions.
It's got zero dependencies so you can paste it into your project without concern.

## Constructing the `CoinSelector`

The main structure is [`CoinSelector`](crate::CoinSelector). To construct it, we specify a list of
candidate UTXOs and a transaction `base_weight`. The `base_weight` includes the recipient outputs
and mandatory inputs (if any).

```rust
use std::str::FromStr;
use bdk_coin_select::{ CoinSelector, Candidate, TXIN_BASE_WEIGHT };
use bitcoin::{ Address, Network, Transaction, TxIn, TxOut };

// You should use miniscript to figure out the satisfaction weight for your coins!
const TR_SATISFACTION_WEIGHT: u32 = 66;
const TR_INPUT_WEIGHT: u32 = TXIN_BASE_WEIGHT + TR_SATISFACTION_WEIGHT;

// The address where we want to send our coins.
let recipient_addr = 
    Address::from_str("tb1pvjf9t34fznr53u5tqhejz4nr69luzkhlvsdsdfq9pglutrpve2xq7hps46")
    .expect("address must be valid")
    .require_network(Network::Testnet)
    .expect("network must match");

let candidates = vec![
    Candidate {
        // How many inputs does this candidate represent. Needed so we can 
        // figure out the weight of the varint that encodes the number of inputs.
        input_count: 1,
        // the value of the input
        value: 1_000_000,
        // the total weight of the input(s). This doesn't include
        weight: TR_INPUT_WEIGHT,
        // wether it's a segwit input. Needed so we know whether to include the
        // segwit header in total weight calculations.
        is_segwit: true
    },
    Candidate {
        // A candidate can represent multiple inputs in the case where you 
        // always want some inputs to be spent together.
        input_count: 2,
        weight: 2*TR_INPUT_WEIGHT,
        value: 3_000_000,
        is_segwit: true
    }
];

let base_tx = Transaction {
    input: vec![],
    // include your recipient outputs here
    output: vec![TxOut {
        value: 900_000,
        script_pubkey: recipient_addr.script_pubkey(),
    }],
    lock_time: bitcoin::absolute::LockTime::from_height(0).unwrap(),
    version: 0x02,
};
let base_weight = base_tx.weight().to_wu() as u32;
println!("base weight: {}", base_weight);

// You can now select coins!
let mut coin_selector = CoinSelector::new(&candidates, base_weight);
coin_selector.select(0);
```

## Change Policy

A change policy determines whether the drain output(s) should be in the final solution. A change
policy is represented by a closure of signature `Fn(&CoinSelector, Target) -> Drain`. We provide 3
built-in change policies; `min_value`, `min_waste` and `min_value_and_waste` (refer to the 
[module-level docs](crate::change_policy) for more).

Typically, to construct a change policy, the [`DrainWeights`] need to be provided. `DrainWeights`
includes two weights. One is the weight of the drain output(s). The other is the weight of spending
the drain output later on (the input weight).

```rust
# use std::str::FromStr;
# use bdk_coin_select::{ CoinSelector, Candidate, DrainWeights, TXIN_BASE_WEIGHT };
# use bitcoin::{ Address, Network, Transaction, TxIn, TxOut };
use bdk_coin_select::change_policy::min_value;
# const TR_SATISFACTION_WEIGHT: u32 = 66;
# const TR_INPUT_WEIGHT: u32 = TXIN_BASE_WEIGHT + TR_SATISFACTION_WEIGHT;
# let base_tx = Transaction {
#     input: vec![],
#     // include your recipient outputs here
#     output: vec![],
#     lock_time: bitcoin::absolute::LockTime::from_height(0).unwrap(),
#     version: 1,
# };
# let base_weight = base_tx.weight().to_wu() as u32;

// The change output that may or may not be included in the final transaction.
let drain_addr =
    Address::from_str("tb1pvjf9t34fznr53u5tqhejz4nr69luzkhlvsdsdfq9pglutrpve2xq7hps46")
    .expect("address must be valid")
    .require_network(Network::Testnet)
    .expect("network must match");

// The drain output(s) may or may not be included in the final tx. We calculate
// the drain weight to include the output length varint weight changes from
// including the drain output(s).
let drain_output_weight = {
    let mut tx_with_drain = base_tx.clone();
    tx_with_drain.output.push(TxOut {
        script_pubkey: drain_addr.script_pubkey(),
        ..Default::default()
    });
    tx_with_drain.weight().to_wu() as u32 - base_weight
};
println!("drain output weight: {}", drain_output_weight);

let drain_weights = DrainWeights {
    output_weight: drain_output_weight,
    spend_weight: TR_INPUT_WEIGHT,
};

// This constructs a change policy that creates change when the change value is
// greater than or equal to the dust limit.
let change_policy = min_value(
    drain_weights,
    drain_addr.script_pubkey().dust_value().to_sat(),
);
```

## Branch and Bound

You can use methods such as [`CoinSelector::select`] to manually select coins, or methods such as
[`CoinSelector::select_until_target_met`] for a rudimentary automatic selection. However, if you 
wish to automatically select coins to optimize for a given metric, [`CoinSelector::run_bnb`] can be
used.

Built-in metrics are provided in the [`metrics`] submodule. Currently, only the 
[`LowestFee`](metrics::LowestFee) metric is considered stable.

```rust
use bdk_coin_select::{ Candidate, CoinSelector, FeeRate, Target };
use bdk_coin_select::metrics::LowestFee;
use bdk_coin_select::change_policy::min_value_and_waste;
# let candidates = [];
# let base_weight = 0;
# let drain_weights = bdk_coin_select::DrainWeights::default();
# let dust_limit = 0;
# let long_term_feerate = FeeRate::default_min_relay_fee();

let mut coin_selector = CoinSelector::new(&candidates, base_weight);

let target = Target {
    feerate: FeeRate::default_min_relay_fee(),
    min_fee: 0,
    value: 210_000,
};

// We use a change policy that introduces a change output if doing so reduces
// the "waste" and that the change output's value is at least that of the 
// `dust_limit`.
let change_policy = min_value_and_waste(
    drain_weights,
    dust_limit,
    long_term_feerate,
);

// This metric minimizes transaction fee. The `long_term_feerate` is used to
// calculate the additional fee from spending the change output in the future.
let metric = LowestFee {
    target,
    long_term_feerate,
    change_policy: &change_policy,
};

// We run the branch and bound algorithm with a max round limit of 100,000.
match coin_selector.run_bnb(metric, 100_000) {
    Err(err) => println!("failed to find a solution: {}", err),
    Ok(score) => {
        println!("we found a solution with score {}", score);

        let selection = coin_selector
            .apply_selection(&candidates)
            .collect::<Vec<_>>();
        let change = change_policy(&coin_selector, target);

        println!("we selected {} inputs", selection.len());
        println!("are we including the change output? {}", change.is_some());
    }
};
```
