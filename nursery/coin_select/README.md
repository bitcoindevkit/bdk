# BDK Coin Selection

`bdk_coin_select` is a tool to help you select inputs for making Bitcoin (ticker: BTC) transactions. It's got zero dependencies so you can pasta it into your project without concern.


## Synopsis

```rust
use bdk_coin_select::{CoinSelector, Candidate, TXIN_BASE_WEIGHT};
use bitcoin::{ Transaction, TxIn };

// You should use miniscript to figure out the satisfaction weight for your coins!
const TR_SATISFACTION_WEIGHT: u32 = 66;
const TR_INPUT_WEIGHT: u32 = TXIN_BASE_WEIGHT + TR_SATISFACTION_WEIGHT;


let candidates = vec![
    Candidate {
        // How many inputs does this candidate represent. Needed so we can figure out the weight
        // of the varint that encodes the number of inputs.
        input_count: 1,
        // the value of the input
        value: 1_000_000,
        // the total weight of the input(s). This doesn't include
        weight: TR_INPUT_WEIGHT,
        // wether it's a segwit input. Needed so we know whether to include the segwit header
        // in total weight calculations.
        is_segwit: true
    },
    Candidate {
        // A candidate can represent multiple inputs in the case where you always want some inputs
        // to be spent together.
        input_count: 2,
        weight: 2*TR_INPUT_WEIGHT,
        value: 3_000_000,
        is_segwit: true
    },
    Candidate {
        input_count: 1,
        weight: TR_INPUT_WEIGHT,
        value: 5_000_000,
        is_segwit: true,
    }
];

let base_weight = Transaction {
    input: vec![],
    output: vec![],
    lock_time: bitcoin::absolute::LockTime::from_height(0).unwrap(),
    version: 1,
}.weight().to_wu() as u32;

println!("base weight: {}", base_weight);

let mut coin_selector = CoinSelector::new(&candidates,base_weight);
```
