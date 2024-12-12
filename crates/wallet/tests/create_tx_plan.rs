use bitcoin::{absolute, relative, secp256k1::Secp256k1, Amount, ScriptBuf, Sequence};
use miniscript::{
    descriptor::{DescriptorPublicKey, KeyMap},
    plan::Assets,
    Descriptor, ForEachKey,
};

use bdk_wallet::error::{CreateTxError, PlanError};
use bdk_wallet::miniscript;
use bdk_wallet::test_utils::*;

fn parse_descriptor(s: &str) -> (Descriptor<DescriptorPublicKey>, KeyMap) {
    <Descriptor<DescriptorPublicKey>>::parse_descriptor(&Secp256k1::new(), s)
        .expect("failed to parse descriptor")
}

#[test]
fn spend_path_from_assets() {
    use std::str::FromStr;

    // Test the plan result for each spending path given the policy
    //  thresh(2,pk(A),and(pk(B),older(6)),and(pk(C),after(630000)))

    let a = "020ffa2c93a3eeed29768a338694da24ad60aa18e06eaf193e7945ad097f21d953";
    let pka = DescriptorPublicKey::from_str(a).unwrap();
    let b = "03f7781a611d88a5f98301ca3de3b33f6c856e4572cea2fefa8b274fe2fc516d3e";
    let pkb = DescriptorPublicKey::from_str(b).unwrap();
    let c = "02bc273a1aca4c50c43572dce6d4857523915bdf09d5f5f4dcedea7c1f9cc41099";
    let pkc = DescriptorPublicKey::from_str(c).unwrap();

    let (desc, _) = parse_descriptor("wsh(thresh(2,pk(020ffa2c93a3eeed29768a338694da24ad60aa18e06eaf193e7945ad097f21d953),snj:and_v(v:pk(03f7781a611d88a5f98301ca3de3b33f6c856e4572cea2fefa8b274fe2fc516d3e),older(6)),snj:and_v(v:pk(02bc273a1aca4c50c43572dce6d4857523915bdf09d5f5f4dcedea7c1f9cc41099),after(850000))))#09tf9qx0");

    // A + B
    let def = desc.at_derivation_index(0).unwrap();
    let assets = Assets::new()
        .add(vec![pka.clone(), pkb.clone()])
        .older(relative::LockTime::from_height(6));
    let plan = def.plan(&assets).unwrap();
    assert!(plan.absolute_timelock.is_none());
    assert_eq!(plan.relative_timelock.unwrap().to_sequence(), Sequence(6));

    // A + C
    let def = desc.at_derivation_index(0).unwrap();
    let assets = Assets::new()
        .add(vec![pka.clone(), pkc.clone()])
        .after(absolute::LockTime::from_consensus(850_000));
    let plan = def.plan(&assets).unwrap();
    assert!(plan.relative_timelock.is_none());
    assert_eq!(plan.absolute_timelock.unwrap().to_consensus_u32(), 850_000);

    // B + C
    let def = desc.at_derivation_index(0).unwrap();
    let assets = Assets::new()
        .add(vec![pkb, pkc])
        .older(relative::LockTime::from_height(6))
        .after(absolute::LockTime::from_consensus(850_000));
    let plan = def.plan(&assets).unwrap();
    assert_eq!(plan.relative_timelock.unwrap().to_sequence(), Sequence(6));
    assert_eq!(plan.absolute_timelock.unwrap().to_consensus_u32(), 850_000);
}

#[test]
fn construct_plan_from_assets() {
    // technically this is tested in rust-miniscript and is only
    // here for demonstration
    let (desc, _) = parse_descriptor(get_test_single_sig_cltv());

    let mut pk = vec![];
    desc.for_each_key(|k| {
        pk.push(k.clone());
        true
    });

    // locktime not met
    let lt = absolute::LockTime::from_consensus(99_999);
    let assets = Assets::new().add(pk.clone()).after(lt);
    let definite_desc = desc.at_derivation_index(0).unwrap();
    definite_desc.plan(&assets).unwrap_err();

    // no keys
    let lt = absolute::LockTime::from_consensus(100_000);
    let assets = Assets::new().after(lt);
    let definite_desc = desc.at_derivation_index(0).unwrap();
    definite_desc.plan(&assets).unwrap_err();

    // assets are sufficient
    let lt = absolute::LockTime::from_consensus(100_000);
    let assets = Assets::new().add(pk).after(lt);
    let definite_desc = desc.at_derivation_index(0).unwrap();
    definite_desc.plan(&assets).unwrap();
}

#[test]
fn create_tx_assets() -> anyhow::Result<()> {
    let abs_locktime = absolute::LockTime::from_consensus(100_000);
    let abs_locktime_t = absolute::LockTime::from_consensus(1734230218);
    let rel_locktime = relative::LockTime::from_consensus(6).unwrap();
    let rel_locktime_144 = relative::LockTime::from_consensus(144).unwrap();

    let default_locktime = absolute::LockTime::from_consensus(2000);
    let default_sequence = Sequence::ENABLE_RBF_NO_LOCKTIME;

    // Test that the assets we pass in are enough to spend outputs defined by a descriptor
    struct TestCase {
        name: &'static str,
        desc: &'static str,
        assets: Option<Assets>,
        exp_cltv: absolute::LockTime,
        exp_sequence: Sequence,
        // whether the result of `finish` is Ok
        exp_result: bool,
    }
    let cases = vec![
        TestCase {
            name: "single sig no assets ok",
            desc: get_test_tr_single_sig(),
            assets: None,
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: true,
        },
        TestCase {
            name: "single sig + cltv",
            desc: get_test_single_sig_cltv(),
            assets: Some(Assets::new().after(abs_locktime)),
            exp_cltv: abs_locktime,
            exp_sequence: default_sequence,
            exp_result: true,
        },
        TestCase {
            name: "single sig + cltv timestamp",
            desc: get_test_single_sig_cltv_timestamp(),
            assets: Some(Assets::new().after(abs_locktime_t)),
            exp_cltv: abs_locktime_t,
            exp_sequence: default_sequence,
            exp_result: true,
        },
        TestCase {
            name: "single sig + csv",
            desc: get_test_single_sig_csv(),
            assets: Some(Assets::new().older(rel_locktime)),
            exp_cltv: default_locktime,
            exp_sequence: Sequence(6),
            exp_result: true,
        },
        TestCase {
            name: "optional csv, no assets ok",
            desc: get_test_a_or_b_plus_csv(),
            assets: None,
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: true,
        },
        TestCase {
            name: "optional csv, use csv",
            desc: get_test_a_or_b_plus_csv(),
            assets: {
                // 2 possible spend paths. we only include the key we
                // intend to sign with
                let (_, keymap) = parse_descriptor(get_test_a_or_b_plus_csv());
                let pk = keymap
                    .keys()
                    .find(|k| k.master_fingerprint().to_string() == "9c5eab64")
                    .unwrap();
                let assets = Assets::new().add(pk.clone()).older(rel_locktime_144);
                Some(assets)
            },
            exp_cltv: default_locktime,
            exp_sequence: Sequence(144),
            exp_result: true,
        },
        TestCase {
            name: "insufficient assets cltv",
            desc: get_test_single_sig_cltv(),
            assets: Some(Assets::new().after(absolute::LockTime::from_consensus(99_999))),
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: false,
        },
        TestCase {
            name: "insufficient assets csv",
            desc: get_test_single_sig_csv(),
            assets: Some(Assets::new().older(relative::LockTime::from_height(5))),
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: false,
        },
        TestCase {
            name: "insufficient assets (no assets)",
            desc: get_test_single_sig_cltv(),
            assets: None,
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: false,
        },
        TestCase {
            name: "wrong locktime (after)",
            desc: get_test_single_sig_csv(),
            assets: Some(Assets::new().after(abs_locktime)),
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: false,
        },
        TestCase {
            name: "wrong locktime (older)",
            desc: get_test_single_sig_cltv(),
            assets: Some(Assets::new().older(rel_locktime)),
            exp_cltv: default_locktime,
            exp_sequence: default_sequence,
            exp_result: false,
        },
    ];

    let recip = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;

    for test in cases {
        let (mut wallet, _) = get_funded_wallet_single(test.desc);
        assert_eq!(wallet.latest_checkpoint().height(), 2_000);
        let mut builder = wallet.build_tx();
        if let Some(assets) = test.assets {
            builder.add_assets(assets);
        }
        builder.add_recipient(recip.clone(), Amount::from_sat(10_000));
        if test.exp_result {
            let psbt = builder.finish().expect("tx builder should not fail");
            assert_eq!(psbt.unsigned_tx.lock_time, test.exp_cltv, "{}", test.name);
            assert_eq!(
                psbt.unsigned_tx.input[0].sequence, test.exp_sequence,
                "{}",
                test.name
            );
        } else {
            let err = builder.finish().expect_err("expected create tx fail");
            assert!(
                matches!(err, CreateTxError::Plan(PlanError::Plan(_))),
                "{}",
                test.name
            );
        }
    }
    Ok(())
}
