#![cfg(feature = "miniscript")]

#[macro_use]
mod common;
use bdk_chain::{
    collections::BTreeMap,
    keychain::{DerivationAdditions, KeychainTxOutIndex},
};

use bitcoin::{secp256k1::Secp256k1, Script, Transaction, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
enum TestKeychain {
    External,
    Internal,
}

fn init_txout_index() -> (
    bdk_chain::keychain::KeychainTxOutIndex<TestKeychain>,
    Descriptor<DescriptorPublicKey>,
    Descriptor<DescriptorPublicKey>,
) {
    let mut txout_index = bdk_chain::keychain::KeychainTxOutIndex::<TestKeychain>::default();

    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    let (external_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
    let (internal_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)").unwrap();

    txout_index.add_keychain(TestKeychain::External, external_descriptor.clone());
    txout_index.add_keychain(TestKeychain::Internal, internal_descriptor.clone());

    (txout_index, external_descriptor, internal_descriptor)
}

fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> Script {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

#[test]
fn test_set_all_derivation_indices() {
    let (mut txout_index, _, _) = init_txout_index();
    let derive_to: BTreeMap<_, _> =
        [(TestKeychain::External, 12), (TestKeychain::Internal, 24)].into();
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to).1.as_inner(),
        &derive_to
    );
    assert_eq!(txout_index.last_revealed_indices(), &derive_to);
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to).1,
        DerivationAdditions::default(),
        "no changes if we set to the same thing"
    );
}

#[test]
fn test_lookahead() {
    let (mut txout_index, external_desc, internal_desc) = init_txout_index();

    // ensure it does not break anything if lookahead is set multiple times
    (0..=10).for_each(|lookahead| txout_index.set_lookahead(&TestKeychain::External, lookahead));
    (0..=20)
        .filter(|v| v % 2 == 0)
        .for_each(|lookahead| txout_index.set_lookahead(&TestKeychain::Internal, lookahead));

    assert_eq!(txout_index.inner().all_spks().len(), 30);

    // given:
    // - external lookahead set to 10
    // - internal lookahead set to 20
    // when:
    // - set external derivation index to value higher than last, but within the lookahead value
    // expect:
    // - scripts cached in spk_txout_index should increase correctly
    // - stored scripts of external keychain should be of expected counts
    for index in (0..20).skip_while(|i| i % 2 == 1) {
        let (revealed_spks, revealed_additions) =
            txout_index.reveal_to_target(&TestKeychain::External, index);
        assert_eq!(
            revealed_spks.collect::<Vec<_>>(),
            vec![(index, spk_at_index(&external_desc, index))],
        );
        assert_eq!(
            revealed_additions.as_inner(),
            &[(TestKeychain::External, index)].into()
        );

        assert_eq!(
            txout_index.inner().all_spks().len(),
            10 /* external lookahead */ +
            20 /* internal lookahead */ +
            index as usize + 1 /* `derived` count */
        );
        assert_eq!(
            txout_index
                .revealed_spks_of_keychain(&TestKeychain::External)
                .count(),
            index as usize + 1,
        );
        assert_eq!(
            txout_index
                .revealed_spks_of_keychain(&TestKeychain::Internal)
                .count(),
            0,
        );
        assert_eq!(
            txout_index
                .unused_spks_of_keychain(&TestKeychain::External)
                .count(),
            index as usize + 1,
        );
        assert_eq!(
            txout_index
                .unused_spks_of_keychain(&TestKeychain::Internal)
                .count(),
            0,
        );
    }

    // given:
    // - internal lookahead is 20
    // - internal derivation index is `None`
    // when:
    // - derivation index is set ahead of current derivation index + lookahead
    // expect:
    // - scripts cached in spk_txout_index should increase correctly, a.k.a. no scripts are skipped
    let (revealed_spks, revealed_additions) =
        txout_index.reveal_to_target(&TestKeychain::Internal, 24);
    assert_eq!(
        revealed_spks.collect::<Vec<_>>(),
        (0..=24)
            .map(|index| (index, spk_at_index(&internal_desc, index)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        revealed_additions.as_inner(),
        &[(TestKeychain::Internal, 24)].into()
    );
    assert_eq!(
        txout_index.inner().all_spks().len(),
        10 /* external lookahead */ +
        20 /* internal lookahead */ +
        20 /* external stored index count */ +
        25 /* internal stored index count */
    );
    assert_eq!(
        txout_index
            .revealed_spks_of_keychain(&TestKeychain::Internal)
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
                        .script_pubkey(),
                    value: 10_000,
                },
                TxOut {
                    script_pubkey: internal_desc
                        .at_derivation_index(internal_index)
                        .script_pubkey(),
                    value: 10_000,
                },
            ],
            ..common::new_tx(external_index)
        };
        assert_eq!(txout_index.scan(&tx), DerivationAdditions::default());
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
                .revealed_spks_of_keychain(&TestKeychain::External)
                .count(),
            last_external_index as usize + 1,
        );
        assert_eq!(
            txout_index
                .revealed_spks_of_keychain(&TestKeychain::Internal)
                .count(),
            last_internal_index as usize + 1,
        );
    }

    // when:
    // - scanning txouts with spks above last stored index
    // expect:
    // - cached scripts count should increase as expected
    // - last stored index should increase as expected
    // TODO!
}

#[test]
fn test_wildcard_derivations() {
    let (mut txout_index, external_desc, _) = init_txout_index();
    let external_spk_0 = external_desc.at_derivation_index(0).script_pubkey();
    let external_spk_16 = external_desc.at_derivation_index(16).script_pubkey();
    let external_spk_26 = external_desc.at_derivation_index(26).script_pubkey();
    let external_spk_27 = external_desc.at_derivation_index(27).script_pubkey();

    // - nothing is derived
    // - unused list is also empty
    //
    // - next_derivation_index() == (0, true)
    // - derive_new() == ((0, <spk>), DerivationAdditions)
    // - next_unused() == ((0, <spk>), DerivationAdditions:is_empty())
    assert_eq!(txout_index.next_index(&TestKeychain::External), (0, true));
    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (0_u32, &external_spk_0));
    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 0)].into());
    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0_u32, &external_spk_0));
    assert_eq!(changeset.as_inner(), &[].into());

    // - derived till 25
    // - used all spks till 15.
    // - used list : [0..=15, 17, 20, 23]
    // - unused list: [16, 18, 19, 21, 22, 24, 25]

    // - next_derivation_index() = (26, true)
    // - derive_new() = ((26, <spk>), DerivationAdditions)
    // - next_unused() == ((16, <spk>), DerivationAdditions::is_empty())
    let _ = txout_index.reveal_to_target(&TestKeychain::External, 25);

    (0..=15)
        .into_iter()
        .chain([17, 20, 23].into_iter())
        .for_each(|index| assert!(txout_index.mark_used(&TestKeychain::External, index)));

    assert_eq!(txout_index.next_index(&TestKeychain::External), (26, true));

    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (26, &external_spk_26));

    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 26)].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (16, &external_spk_16));
    assert_eq!(changeset.as_inner(), &[].into());

    // - Use all the derived till 26.
    // - next_unused() = ((27, <spk>), DerivationAdditions)
    (0..=26).into_iter().for_each(|index| {
        txout_index.mark_used(&TestKeychain::External, index);
    });

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (27, &external_spk_27));
    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 27)].into());
}

#[test]
fn test_non_wildcard_derivations() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::default();

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (no_wildcard_descriptor, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0)").unwrap();
    let external_spk = no_wildcard_descriptor
        .at_derivation_index(0)
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
    assert_eq!(spk, (0, &external_spk));
    assert_eq!(changeset.as_inner(), &[(TestKeychain::External, 0)].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0, &external_spk));
    assert_eq!(changeset.as_inner(), &[].into());

    // given:
    // - the non-wildcard descriptor already has a stored and used script
    // expect:
    // - next derivation index should not be new
    // - derive new and next unused should return the old script
    // - store_up_to should not panic and return empty additions
    assert_eq!(txout_index.next_index(&TestKeychain::External), (0, false));
    txout_index.mark_used(&TestKeychain::External, 0);

    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (0, &external_spk));
    assert_eq!(changeset.as_inner(), &[].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0, &external_spk));
    assert_eq!(changeset.as_inner(), &[].into());
    let (revealed_spks, revealed_additions) =
        txout_index.reveal_to_target(&TestKeychain::External, 200);
    assert_eq!(revealed_spks.count(), 0);
    assert!(revealed_additions.is_empty());
}
