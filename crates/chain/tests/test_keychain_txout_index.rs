#![cfg(feature = "miniscript")]

#[macro_use]
mod common;
use bdk_chain::{
    collections::BTreeMap,
    indexed_tx_graph::Indexer,
    keychain::{self, KeychainTxOutIndex},
    Append,
};

use bitcoin::{secp256k1::Secp256k1, Amount, OutPoint, ScriptBuf, Transaction, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
enum TestKeychain {
    External,
    Internal,
}

fn init_txout_index(
    lookahead: u32,
) -> (
    bdk_chain::keychain::KeychainTxOutIndex<TestKeychain>,
    Descriptor<DescriptorPublicKey>,
    Descriptor<DescriptorPublicKey>,
) {
    let mut txout_index = bdk_chain::keychain::KeychainTxOutIndex::<TestKeychain>::new(lookahead);

    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    let (external_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
    let (internal_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)").unwrap();

    txout_index.add_keychain(TestKeychain::External, external_descriptor.clone());
    txout_index.add_keychain(TestKeychain::Internal, internal_descriptor.clone());

    (txout_index, external_descriptor, internal_descriptor)
}

fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

#[test]
fn test_set_all_derivation_indices() {
    use bdk_chain::indexed_tx_graph::Indexer;

    let (mut txout_index, _, _) = init_txout_index(0);
    let derive_to: BTreeMap<_, _> =
        [(TestKeychain::External, 12), (TestKeychain::Internal, 24)].into();
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to).1.as_inner(),
        &derive_to
    );
    assert_eq!(txout_index.last_revealed_indices(), &derive_to);
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to).1,
        keychain::ChangeSet::default(),
        "no changes if we set to the same thing"
    );
    assert_eq!(txout_index.initial_changeset().as_inner(), &derive_to);
}

#[test]
fn test_lookahead() {
    let (mut txout_index, external_desc, internal_desc) = init_txout_index(10);

    // given:
    // - external lookahead set to 10
    // when:
    // - set external derivation index to value higher than last, but within the lookahead value
    // expect:
    // - scripts cached in spk_txout_index should increase correctly
    // - stored scripts of external keychain should be of expected counts
    for index in (0..20).skip_while(|i| i % 2 == 1) {
        let (revealed_spks, revealed_changeset) =
            txout_index.reveal_to_target(&TestKeychain::External, index);
        assert_eq!(
            revealed_spks.collect::<Vec<_>>(),
            vec![(index, spk_at_index(&external_desc, index))],
        );
        assert_eq!(
            revealed_changeset.as_inner(),
            &[(TestKeychain::External, index)].into()
        );

        assert_eq!(
            txout_index.inner().all_spks().len(),
            10 /* external lookahead */ +
            10 /* internal lookahead */ +
            index as usize + 1 /* `derived` count */
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(&TestKeychain::External)
                .count(),
            index as usize + 1,
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(&TestKeychain::Internal)
                .count(),
            0,
        );
        assert_eq!(
            txout_index
                .unused_keychain_spks(&TestKeychain::External)
                .count(),
            index as usize + 1,
        );
        assert_eq!(
            txout_index
                .unused_keychain_spks(&TestKeychain::Internal)
                .count(),
            0,
        );
    }

    // given:
    // - internal lookahead is 10
    // - internal derivation index is `None`
    // when:
    // - derivation index is set ahead of current derivation index + lookahead
    // expect:
    // - scripts cached in spk_txout_index should increase correctly, a.k.a. no scripts are skipped
    let (revealed_spks, revealed_changeset) =
        txout_index.reveal_to_target(&TestKeychain::Internal, 24);
    assert_eq!(
        revealed_spks.collect::<Vec<_>>(),
        (0..=24)
            .map(|index| (index, spk_at_index(&internal_desc, index)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        revealed_changeset.as_inner(),
        &[(TestKeychain::Internal, 24)].into()
    );
    assert_eq!(
        txout_index.inner().all_spks().len(),
        10 /* external lookahead */ +
        10 /* internal lookahead */ +
        20 /* external stored index count */ +
        25 /* internal stored index count */
    );
    assert_eq!(
        txout_index
            .revealed_keychain_spks(&TestKeychain::Internal)
            .count(),
        25,
    );

    // ensure derivation indices are expected for each keychain
    let last_external_index = txout_index
        .last_revealed_index(&TestKeychain::External)
        .expect("already derived");
    let last_internal_index = txout_index
        .last_revealed_index(&TestKeychain::Internal)
        .expect("already derived");
    assert_eq!(last_external_index, 19);
    assert_eq!(last_internal_index, 24);

    // when:
    // - scanning txouts with spks within stored indexes
    // expect:
    // - no changes to stored index counts
    let external_iter = 0..=last_external_index;
    let internal_iter = last_internal_index - last_external_index..=last_internal_index;
    for (external_index, internal_index) in external_iter.zip(internal_iter) {
        let tx = Transaction {
            output: vec![
                TxOut {
                    script_pubkey: external_desc
                        .at_derivation_index(external_index)
                        .unwrap()
                        .script_pubkey(),
                    value: Amount::from_int_btc(10_000),
                },
                TxOut {
                    script_pubkey: internal_desc
                        .at_derivation_index(internal_index)
                        .unwrap()
                        .script_pubkey(),
                    value: Amount::from_int_btc(10_000),
                },
            ],
            ..common::new_tx(external_index)
        };
        assert_eq!(txout_index.index_tx(&tx), keychain::ChangeSet::default());
        assert_eq!(
            txout_index.last_revealed_index(&TestKeychain::External),
            Some(last_external_index)
        );
        assert_eq!(
            txout_index.last_revealed_index(&TestKeychain::Internal),
            Some(last_internal_index)
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(&TestKeychain::External)
                .count(),
            last_external_index as usize + 1,
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(&TestKeychain::Internal)
                .count(),
            last_internal_index as usize + 1,
        );
    }
}

// when:
// - scanning txouts with spks above last stored index
// expect:
// - last revealed index should increase as expected
// - last used index should change as expected
#[test]
fn test_scan_with_lookahead() {
    let (mut txout_index, external_desc, _) = init_txout_index(10);

    let spks: BTreeMap<u32, ScriptBuf> = [0, 10, 20, 30]
        .into_iter()
        .map(|i| {
            (
                i,
                external_desc
                    .at_derivation_index(i)
                    .unwrap()
                    .script_pubkey(),
            )
        })
        .collect();

    for (&spk_i, spk) in &spks {
        let op = OutPoint::new(h!("fake tx"), spk_i);
        let txout = TxOut {
            script_pubkey: spk.clone(),
            value: Amount::from_int_btc(0),
        };

        let changeset = txout_index.index_txout(op, &txout);
        assert_eq!(
            changeset.as_inner(),
            &[(TestKeychain::External, spk_i)].into()
        );
        assert_eq!(
            txout_index.last_revealed_index(&TestKeychain::External),
            Some(spk_i)
        );
        assert_eq!(
            txout_index.last_used_index(&TestKeychain::External),
            Some(spk_i)
        );
    }

    // now try with index 41 (lookahead surpassed), we expect that the txout to not be indexed
    let spk_41 = external_desc
        .at_derivation_index(41)
        .unwrap()
        .script_pubkey();
    let op = OutPoint::new(h!("fake tx"), 41);
    let txout = TxOut {
        script_pubkey: spk_41,
        value: Amount::from_int_btc(0),
    };
    let changeset = txout_index.index_txout(op, &txout);
    assert!(changeset.is_empty());
}

#[test]
#[rustfmt::skip]
fn test_wildcard_derivations() {
    let (mut txout_index, external_desc, _) = init_txout_index(0);
    let external_spk_0 = external_desc.at_derivation_index(0).unwrap().script_pubkey();
    let external_spk_16 = external_desc.at_derivation_index(16).unwrap().script_pubkey();
    let external_spk_26 = external_desc.at_derivation_index(26).unwrap().script_pubkey();
    let external_spk_27 = external_desc.at_derivation_index(27).unwrap().script_pubkey();

    // - nothing is derived
    // - unused list is also empty
    //
    // - next_derivation_index() == (0, true)
    // - derive_new() == ((0, <spk>), keychain::ChangeSet)
    // - next_unused() == ((0, <spk>), keychain::ChangeSet:is_empty())
    assert_eq!(txout_index.next_index(&TestKeychain::External), (0, true));
    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (0_u32, external_spk_0.as_script()));
    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 0)].into());
    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0_u32, external_spk_0.as_script()));
    assert_eq!(changeset.as_inner(), &[].into());

    // - derived till 25
    // - used all spks till 15.
    // - used list : [0..=15, 17, 20, 23]
    // - unused list: [16, 18, 19, 21, 22, 24, 25]

    // - next_derivation_index() = (26, true)
    // - derive_new() = ((26, <spk>), keychain::ChangeSet)
    // - next_unused() == ((16, <spk>), keychain::ChangeSet::is_empty())
    let _ = txout_index.reveal_to_target(&TestKeychain::External, 25);

    (0..=15)
        .chain([17, 20, 23])
        .for_each(|index| assert!(txout_index.mark_used(TestKeychain::External, index)));

    assert_eq!(txout_index.next_index(&TestKeychain::External), (26, true));

    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (26, external_spk_26.as_script()));

    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 26)].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (16, external_spk_16.as_script()));
    assert_eq!(changeset.as_inner(), &[].into());

    // - Use all the derived till 26.
    // - next_unused() = ((27, <spk>), keychain::ChangeSet)
    (0..=26).for_each(|index| {
        txout_index.mark_used(TestKeychain::External, index);
    });

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (27, external_spk_27.as_script()));
    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 27)].into());
}

#[test]
fn test_non_wildcard_derivations() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(0);

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (no_wildcard_descriptor, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0)").unwrap();
    let external_spk = no_wildcard_descriptor
        .at_derivation_index(0)
        .unwrap()
        .script_pubkey();

    txout_index.add_keychain(TestKeychain::External, no_wildcard_descriptor);

    // given:
    // - `txout_index` with no stored scripts
    // expect:
    // - next derivation index should be new
    // - when we derive a new script, script @ index 0
    // - when we get the next unused script, script @ index 0
    assert_eq!(txout_index.next_index(&TestKeychain::External), (0, true));
    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 0)].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(changeset.as_inner(), &[].into());

    // given:
    // - the non-wildcard descriptor already has a stored and used script
    // expect:
    // - next derivation index should not be new
    // - derive new and next unused should return the old script
    // - store_up_to should not panic and return empty changeset
    assert_eq!(txout_index.next_index(&TestKeychain::External), (0, false));
    txout_index.mark_used(TestKeychain::External, 0);

    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(changeset.as_inner(), &[].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(changeset.as_inner(), &[].into());
    let (revealed_spks, revealed_changeset) =
        txout_index.reveal_to_target(&TestKeychain::External, 200);
    assert_eq!(revealed_spks.count(), 0);
    assert!(revealed_changeset.is_empty());

    // we check that spks_of_keychain returns a SpkIterator with just one element
    assert_eq!(
        txout_index
            .revealed_keychain_spks(&TestKeychain::External)
            .count(),
        1,
    );
}
