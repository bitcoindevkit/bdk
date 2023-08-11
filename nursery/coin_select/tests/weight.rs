#![allow(clippy::zero_prefixed_literal)]

use std::str::FromStr;

use bdk_coin_select::{Candidate, CoinSelector, Drain};
use bitcoin::{absolute::Height, consensus::Decodable, Address, ScriptBuf, Transaction, TxOut};

fn hex_val(c: u8) -> u8 {
    match c {
        b'A'..=b'F' => c - b'A' + 10,
        b'a'..=b'f' => c - b'a' + 10,
        b'0'..=b'9' => c - b'0',
        _ => panic!("invalid"),
    }
}

// Appears that Transaction has no from_str so I had to roll my own hex decoder
pub fn hex_decode(hex: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(hex.len() * 2);
    for hex_byte in hex.as_bytes().chunks(2) {
        bytes.push(hex_val(hex_byte[0]) << 4 | hex_val(hex_byte[1]))
    }
    bytes
}

#[test]
fn segwit_one_input_one_output() {
    // FROM https://mempool.space/tx/e627fbb7f775a57fd398bf9b150655d4ac3e1f8afed4255e74ee10d7a345a9cc
    let mut tx_bytes = hex_decode("01000000000101b2ec00fd7d3f2c89eb27e3e280960356f69fc88a324a4bca187dd4b020aa36690000000000ffffffff01d0bb9321000000001976a9141dc94fe723f43299c6187094b1dc5a032d47b06888ac024730440220669b764de7e9dcedcba6d6d57c8c761be2acc4e1a66938ceecacaa6d494f582d02202641df89d1758eeeed84290079dd9ad36611c73cd9e381dd090b83f5e5b1422e012103f6544e4ffaff4f8649222003ada5d74bd6d960162bcd85af2b619646c8c45a5298290c00");
    let mut cursor = std::io::Cursor::new(&mut tx_bytes);
    let mut tx = Transaction::consensus_decode(&mut cursor).unwrap();
    let input_values = vec![563_336_755];
    let inputs = core::mem::take(&mut tx.input);

    let candidates = inputs
        .iter()
        .zip(input_values)
        .map(|(txin, value)| Candidate {
            value,
            weight: txin.segwit_weight() as u32,
            input_count: 1,
            is_segwit: true,
        })
        .collect::<Vec<_>>();

    let mut coin_selector = CoinSelector::new(&candidates, tx.weight().to_wu() as u32);
    coin_selector.select_all();

    assert_eq!(coin_selector.weight(0), 449);
    assert_eq!(
        (coin_selector
            .implied_feerate(tx.output[0].value, Drain::none())
            .as_sat_vb()
            * 10.0)
            .round(),
        60.2 * 10.0
    );
}

#[test]
fn segwit_two_inputs_one_output() {
    // FROM https://mempool.space/tx/37d2883bdf1b4c110b54cb624d36ab6a30140f8710ed38a52678260a7685e708
    let mut tx_bytes = hex_decode("020000000001021edcae5160b1ba2370a45ea9342b4c883a8941274539612bddf1c379ba7ecf180700000000ffffffff5c85e19bf4f0e293c0d5f9665cb05d2a55d8bba959edc5ef02075f6a1eb9fc120100000000ffffffff0168ce3000000000001976a9145ff742d992276a1f46e5113dde7382896ff86e2a88ac0247304402202e588db55227e0c24db7f07b65f221ebcae323fb595d13d2e1c360b773d809b0022008d2f57a618bd346cfd031549a3971f22464e3e3308cee340a976f1b47a96f0b012102effbcc87e6c59b810c2fa20b0bc3eb909a20b40b25b091cf005d416b85db8c8402483045022100bdc115b86e9c863279132b4808459cf9b266c8f6a9c14a3dfd956986b807e3320220265833b85197679687c5d5eed1b2637489b34249d44cf5d2d40bc7b514181a51012102077741a668889ce15d59365886375aea47a7691941d7a0d301697edbc773b45b00000000");
    let mut cursor = std::io::Cursor::new(&mut tx_bytes);
    let mut tx = Transaction::consensus_decode(&mut cursor).unwrap();
    let input_values = vec![003_194_967, 000_014_068];
    let inputs = core::mem::take(&mut tx.input);

    let candidates = inputs
        .iter()
        .zip(input_values)
        .map(|(txin, value)| Candidate {
            value,
            weight: txin.segwit_weight() as u32,
            input_count: 1,
            is_segwit: true,
        })
        .collect::<Vec<_>>();

    let mut coin_selector = CoinSelector::new(&candidates, tx.weight().to_wu() as u32);
    coin_selector.select_all();

    assert_eq!(coin_selector.weight(0), 721);
    assert_eq!(
        (coin_selector
            .implied_feerate(tx.output[0].value, Drain::none())
            .as_sat_vb()
            * 10.0)
            .round(),
        58.1 * 10.0
    );
}

#[test]
fn legacy_three_inputs() {
    // FROM https://mempool.space/tx/5f231df4f73694b3cca9211e336451c20dab136e0a843c2e3166cdcb093e91f4
    let mut tx_bytes = hex_decode("0100000003fe785783e14669f638ba902c26e8e3d7036fb183237bc00f8a10542191c7171300000000fdfd00004730440220418996f20477d143d02ad47e74e5949641b6c2904159ab7c592d2cfc659f9bd802205b18f18ac86b714971f84a8b74a4cb14ad5c1a5b9d0d939bb32c6ae4032f4ea10148304502210091296ff8dd87b5ebfc3d47cb82cfe4750d52c544a2b88a85970354a4d0d4b1db022069632067ee6f30f06145f649bc76d5e5d5e6404dbe985e006fcde938f778c297014c695221030502b8ade694d57a6e86998180a64f4ce993372830dc796c3d561ad8b2a504de210272b68e1c037c4630eff7ea5858640cc0748e36f5de82fb38529ef1fd0a89670d2103ba0544a3a2aa9f2314022760b78b5c833aebf6f88468a089550f93834a2886ed53aeffffffff7e048a7c53a8af656e24442c65fe4c4299b1494f6c7579fe0fd9fa741ce83e3279000000fc004730440220018fa343acccd048ed8f8f179e1b6ae27435a41b5fb2c1d96a5a772777acc6dc022074783814f2100c6fc4d4c976f941212be50825814502ca0cbe3f929db789979e0147304402206373f01b73fb09876d0f5ee3087e0614cab3be249934bc2b7eb64ee67f53dc8302200b50f8a327020172b82aaba7480c77ecf07bb32322a05f4afbc543aa97d2fde8014c69522103039d906b2494e310f6c7774c98618be552720d04781e073dd3ff25d5906f22662103d82026baa529619b103ec6341d548a7eb6d924061a8469a7416155513a3071c12102e452bc4aa726d44646ba80db70465683b30efde282a19aa35c6029ae8925df5e53aeffffffffef80f0b1cc543de4f73d59c02a3c575ae5d0af17c1e11e6be7abe3325c777507ad000000fdfd00004730440220220fee11bf836621a11a8ea9100a4600c109c13895f11468d3e2062210c5481902201c5c8a462175538e87b8248e1ed3927c3a461c66d1b46215641c875e86eb22c4014830450221008d2de8c2f20a720129c372791e595b9602b1a9bce99618497aec5266148ffc1302203a493359d700ed96323f8805ed03e909959ff0f22eff359028db6861486b1555014c6952210374a4add33567f09967592c5bcdc3db421fdbba67bac4636328f96d941da31bd221039636c2ffac90afb7499b16e265078113dfb2d77b54270e37353217c9eaeaf3052103d0bcea6d10cdd2f16018ea71572631708e26f457f67cda36a7f816a87f7791d253aeffffffff04977261000000000016001470385d054721987f41521648d7b2f5c77f735d6bee92030000000000225120d0cda1b675a0b369964cbfa381721aae3549dd2c9c6f2cf71ff67d5bc277afd3f2aaf30000000000160014ed2d41ba08313dbb2630a7106b2fedafc14aa121d4f0c70000000000220020e5c7c00d174631d2d1e365d6347b016fb87b6a0c08902d8e443989cb771fa7ec00000000");
    let mut cursor = std::io::Cursor::new(&mut tx_bytes);
    let mut tx = Transaction::consensus_decode(&mut cursor).unwrap();
    let orig_weight = tx.weight();
    let input_values = vec![022_680_000, 006_558_175, 006_558_200];
    let inputs = core::mem::take(&mut tx.input);
    let candidates = inputs
        .iter()
        .zip(input_values)
        .map(|(txin, value)| Candidate {
            value,
            weight: txin.legacy_weight() as u32,
            input_count: 1,
            is_segwit: false,
        })
        .collect::<Vec<_>>();

    let mut coin_selector = CoinSelector::new(&candidates, tx.weight().to_wu() as u32);
    coin_selector.select_all();

    assert_eq!(coin_selector.weight(0), orig_weight.to_wu() as u32);
    assert_eq!(
        (coin_selector
            .implied_feerate(tx.output.iter().map(|o| o.value).sum(), Drain::none())
            .as_sat_vb()
            * 10.0)
            .round(),
        99.2 * 10.0
    );
}

#[test]
fn legacy_three_inputs_one_segwit() {
    // FROM https://mempool.space/tx/5f231df4f73694b3cca9211e336451c20dab136e0a843c2e3166cdcb093e91f4
    // Except we change the middle input to segwit
    let mut tx_bytes = hex_decode("0100000003fe785783e14669f638ba902c26e8e3d7036fb183237bc00f8a10542191c7171300000000fdfd00004730440220418996f20477d143d02ad47e74e5949641b6c2904159ab7c592d2cfc659f9bd802205b18f18ac86b714971f84a8b74a4cb14ad5c1a5b9d0d939bb32c6ae4032f4ea10148304502210091296ff8dd87b5ebfc3d47cb82cfe4750d52c544a2b88a85970354a4d0d4b1db022069632067ee6f30f06145f649bc76d5e5d5e6404dbe985e006fcde938f778c297014c695221030502b8ade694d57a6e86998180a64f4ce993372830dc796c3d561ad8b2a504de210272b68e1c037c4630eff7ea5858640cc0748e36f5de82fb38529ef1fd0a89670d2103ba0544a3a2aa9f2314022760b78b5c833aebf6f88468a089550f93834a2886ed53aeffffffff7e048a7c53a8af656e24442c65fe4c4299b1494f6c7579fe0fd9fa741ce83e3279000000fc004730440220018fa343acccd048ed8f8f179e1b6ae27435a41b5fb2c1d96a5a772777acc6dc022074783814f2100c6fc4d4c976f941212be50825814502ca0cbe3f929db789979e0147304402206373f01b73fb09876d0f5ee3087e0614cab3be249934bc2b7eb64ee67f53dc8302200b50f8a327020172b82aaba7480c77ecf07bb32322a05f4afbc543aa97d2fde8014c69522103039d906b2494e310f6c7774c98618be552720d04781e073dd3ff25d5906f22662103d82026baa529619b103ec6341d548a7eb6d924061a8469a7416155513a3071c12102e452bc4aa726d44646ba80db70465683b30efde282a19aa35c6029ae8925df5e53aeffffffffef80f0b1cc543de4f73d59c02a3c575ae5d0af17c1e11e6be7abe3325c777507ad000000fdfd00004730440220220fee11bf836621a11a8ea9100a4600c109c13895f11468d3e2062210c5481902201c5c8a462175538e87b8248e1ed3927c3a461c66d1b46215641c875e86eb22c4014830450221008d2de8c2f20a720129c372791e595b9602b1a9bce99618497aec5266148ffc1302203a493359d700ed96323f8805ed03e909959ff0f22eff359028db6861486b1555014c6952210374a4add33567f09967592c5bcdc3db421fdbba67bac4636328f96d941da31bd221039636c2ffac90afb7499b16e265078113dfb2d77b54270e37353217c9eaeaf3052103d0bcea6d10cdd2f16018ea71572631708e26f457f67cda36a7f816a87f7791d253aeffffffff04977261000000000016001470385d054721987f41521648d7b2f5c77f735d6bee92030000000000225120d0cda1b675a0b369964cbfa381721aae3549dd2c9c6f2cf71ff67d5bc277afd3f2aaf30000000000160014ed2d41ba08313dbb2630a7106b2fedafc14aa121d4f0c70000000000220020e5c7c00d174631d2d1e365d6347b016fb87b6a0c08902d8e443989cb771fa7ec00000000");
    let mut cursor = std::io::Cursor::new(&mut tx_bytes);
    let mut tx = Transaction::consensus_decode(&mut cursor).unwrap();
    tx.input[1].script_sig = ScriptBuf::default();
    tx.input[1].witness = vec![
        // semi-realistic p2wpkh spend
        hex_decode("3045022100bdc115b86e9c863279132b4808459cf9b266c8f6a9c14a3dfd956986b807e3320220265833b85197679687c5d5eed1b2637489b34249d44cf5d2d40bc7b514181a5101"),
        hex_decode("02077741a668889ce15d59365886375aea47a7691941d7a0d301697edbc773b45b"),
    ].into();
    let orig_weight = tx.weight();
    let input_values = vec![022_680_000, 006_558_175, 006_558_200];
    let inputs = core::mem::take(&mut tx.input);
    let candidates = inputs
        .iter()
        .zip(input_values)
        .enumerate()
        .map(|(i, (txin, value))| {
            let is_segwit = i == 1;
            Candidate {
                value,
                weight: if is_segwit {
                    txin.segwit_weight()
                } else {
                    txin.legacy_weight()
                } as u32,
                input_count: 1,
                is_segwit,
            }
        })
        .collect::<Vec<_>>();

    let mut coin_selector = CoinSelector::new(&candidates, tx.weight().to_wu() as u32);
    coin_selector.select_all();

    assert_eq!(coin_selector.weight(0), orig_weight.to_wu() as u32);
}

/// Ensure that `fund_outputs` caculates the same `base_weight` as `rust-bitcoin`.
///
/// We test it with 3 different output counts (resulting in different varint output-count weights).
#[test]
fn fund_outputs() {
    let txo = TxOut {
        script_pubkey: Address::from_str("bc1q4hym5spvze5d4wand9mf9ed7ku00kg6cv3h9ct")
            .expect("must parse address")
            .assume_checked()
            .script_pubkey(),
        value: 50_000,
    };
    let txo_weight = txo.weight() as u32;

    let output_counts: &[usize] = &[0x01, 0xfd, 0x01_0000];

    for &output_count in output_counts {
        let weight_from_fund_outputs =
            CoinSelector::fund_outputs(&[], (0..=output_count).map(|_| txo_weight)).weight(0);

        let exp_weight = Transaction {
            version: 0,
            lock_time: bitcoin::absolute::LockTime::Blocks(Height::ZERO),
            input: Vec::new(),
            output: (0..=output_count).map(|_| txo.clone()).collect(),
        }
        .weight()
        .to_wu() as u32;

        assert_eq!(weight_from_fund_outputs, exp_weight);
    }
}
