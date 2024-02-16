#![cfg(feature = "miniscript")]

extern crate core;

#[macro_use]
mod common;

use bdk_chain::{
    collections::BTreeMap,
    indexed_tx_graph::Indexer,
    keychain::{self, ChangeSet, KeychainTxOutIndex},
    Append,
};

use bdk_chain::keychain::AddKeychainError;
use bitcoin::{secp256k1::Secp256k1, OutPoint, ScriptBuf, Transaction, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::common::{descriptor_ids, DESCRIPTORS};

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
enum TestKeychain {
    External,
    Internal,
    Other,
}

struct TestIndex {
    index: bdk_chain::keychain::KeychainTxOutIndex<TestKeychain>,
    external_descriptor: (Descriptor<DescriptorPublicKey>, [u8; 8]),
    internal_descriptor: (Descriptor<DescriptorPublicKey>, [u8; 8]),
}

fn init_txout_index(lookahead: u32) -> TestIndex {
    let mut txout_index = bdk_chain::keychain::KeychainTxOutIndex::<TestKeychain>::new(lookahead);

    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    let (external_descriptor, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[0]).unwrap();
    let (internal_descriptor, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[1]).unwrap();

    _ = txout_index
        .add_keychain(TestKeychain::External, external_descriptor.clone())
        .expect("no dup keychain or descriptor");
    _ = txout_index
        .add_keychain(TestKeychain::Internal, internal_descriptor.clone())
        .expect("no dup keychain or descriptor");

    TestIndex {
        index: txout_index,
        external_descriptor: (external_descriptor, descriptor_ids(0)),
        internal_descriptor: (internal_descriptor, descriptor_ids(1)),
    }
}

fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

#[test]
fn append_keychain_derivation_indices() {
    let mut lhs_di = BTreeMap::<[u8; 8], u32>::default();
    let mut rhs_di = BTreeMap::<[u8; 8], u32>::default();
    lhs_di.insert(descriptor_ids(0), 7);
    lhs_di.insert(descriptor_ids(1), 0);
    rhs_di.insert(descriptor_ids(1), 3);
    rhs_di.insert(descriptor_ids(1), 5);
    lhs_di.insert(descriptor_ids(2), 3);
    rhs_di.insert(descriptor_ids(3), 4);

    let mut lhs = ChangeSet {
        keychains_added: BTreeMap::<(), _>::new(),
        last_revealed: lhs_di,
    };
    let rhs = ChangeSet {
        keychains_added: BTreeMap::<(), _>::new(),
        last_revealed: rhs_di,
    };
    lhs.append(rhs);

    // Existing index doesn't update if the new index in `other` is lower than `self`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids(0)), Some(&7));
    // Existing index updates if the new index in `other` is higher than `self`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids(1)), Some(&5));
    // Existing index is unchanged if keychain doesn't exist in `other`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids(2)), Some(&3));
    // New keychain gets added if the keychain is in `other` but not in `self`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids(3)), Some(&4));
}

// test panic when adding existing keychain with different descriptor
#[test]
#[should_panic]
fn test_insert_different_desc_existing_keychain() {
    let TestIndex {
        index: mut txout_index,
        external_descriptor: (external_descriptor, _),
        internal_descriptor: (internal_descriptor, _),
    } = init_txout_index(0);
    assert_eq!(
        txout_index.keychains().collect::<Vec<_>>(),
        vec![
            (TestKeychain::External, &external_descriptor),
            (TestKeychain::Internal, &internal_descriptor)
        ]
    );

    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    let (other_descriptor, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[2]).unwrap();

    let changeset = ChangeSet {
        keychains_added: [(TestKeychain::External, other_descriptor.clone())].into(),
        last_revealed: [].into(),
    };
    txout_index.apply_changeset(changeset);
}

// test panic when adding existing descriptor with different keychain
#[test]
#[should_panic]
fn test_insert_different_keychain_existing_descriptor() {
    let TestIndex {
        index: mut txout_index,
        external_descriptor: (external_descriptor, _),
        internal_descriptor: (internal_descriptor, _),
    } = init_txout_index(0);
    assert_eq!(
        txout_index.keychains().collect::<Vec<_>>(),
        vec![
            (TestKeychain::External, &external_descriptor),
            (TestKeychain::Internal, &internal_descriptor)
        ]
    );

    let changeset = ChangeSet {
        keychains_added: [(TestKeychain::Other, internal_descriptor.clone())].into(),
        last_revealed: [].into(),
    };
    txout_index.apply_changeset(changeset);
}

#[test]
fn test_set_all_derivation_indices() {
    use bdk_chain::indexed_tx_graph::Indexer;

    let TestIndex {
        index: mut txout_index,
        external_descriptor: (_, external_id),
        internal_descriptor: (_, internal_id),
    } = init_txout_index(0);
    let derive_to: BTreeMap<_, _> =
        [(TestKeychain::External, 12), (TestKeychain::Internal, 24)].into();
    let last_revealed: BTreeMap<_, _> = [(external_id, 12), (internal_id, 24)].into();
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to).1,
        ChangeSet {
            keychains_added: BTreeMap::new(),
            last_revealed: last_revealed.clone()
        }
    );
    assert_eq!(txout_index.last_revealed_indices(), derive_to);
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to).1,
        keychain::ChangeSet::default(),
        "no changes if we set to the same thing"
    );
    assert_eq!(txout_index.initial_changeset().last_revealed, last_revealed);
}

#[test]
fn test_lookahead() {
    let TestIndex {
        index: mut txout_index,
        external_descriptor: (external_desc, external_desc_id),
        internal_descriptor: (internal_desc, internal_desc_id),
    } = init_txout_index(10);

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
            &revealed_changeset.last_revealed,
            &[(external_desc_id, index)].into()
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
        &revealed_changeset.last_revealed,
        &[(internal_desc_id, 24)].into()
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
                    value: 10_000,
                },
                TxOut {
                    script_pubkey: internal_desc
                        .at_derivation_index(internal_index)
                        .unwrap()
                        .script_pubkey(),
                    value: 10_000,
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
    let TestIndex {
        index: mut txout_index,
        external_descriptor: (external_desc, external_desc_id),
        ..
    } = init_txout_index(10);

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
            value: 0,
        };

        let changeset = txout_index.index_txout(op, &txout);
        assert_eq!(
            &changeset.last_revealed,
            &[(external_desc_id, spk_i)].into()
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
        value: 0,
    };
    let changeset = txout_index.index_txout(op, &txout);
    assert!(changeset.is_empty());
}

#[test]
#[rustfmt::skip]
fn test_wildcard_derivations() {
    let TestIndex { index: mut txout_index, external_descriptor: (external_desc, external_desc_id), .. } = init_txout_index(0);
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
    assert_eq!(&changeset.last_revealed, &[(external_desc_id, 0)].into());
    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0_u32, external_spk_0.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());

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

    assert_eq!(&changeset.last_revealed, &[(external_desc_id, 26)].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (16, external_spk_16.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());

    // - Use all the derived till 26.
    // - next_unused() = ((27, <spk>), keychain::ChangeSet)
    (0..=26).for_each(|index| {
        txout_index.mark_used(TestKeychain::External, index);
    });

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (27, external_spk_27.as_script()));
    assert_eq!(&changeset.last_revealed, &[(external_desc_id, 27)].into());
}

#[test]
fn test_non_wildcard_derivations() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(0);

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (no_wildcard_descriptor, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[6]).unwrap();
    let no_wildcard_descriptor_id = descriptor_ids(6);
    let external_spk = no_wildcard_descriptor
        .at_derivation_index(0)
        .unwrap()
        .script_pubkey();

    _ = txout_index
        .add_keychain(TestKeychain::External, no_wildcard_descriptor)
        .expect("no dup keychain or descriptor");

    // given:
    // - `txout_index` with no stored scripts
    // expect:
    // - next derivation index should be new
    // - when we derive a new script, script @ index 0
    // - when we get the next unused script, script @ index 0
    assert_eq!(txout_index.next_index(&TestKeychain::External), (0, true));
    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(
        &changeset.last_revealed,
        &[(no_wildcard_descriptor_id, 0)].into()
    );

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());

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
    assert_eq!(&changeset.last_revealed, &[].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External);
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());
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

// Test error when adding duplicate keychain with different descriptor
#[test]
fn test_add_duplicate_keychain_error() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(0);

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (descriptor_0, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[0]).unwrap();

    let (descriptor_1, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[1]).unwrap();

    _ = txout_index
        .add_keychain(TestKeychain::External, descriptor_0.clone())
        .expect("no dup keychain or descriptor");

    let add_keychain_result = txout_index.add_keychain(TestKeychain::External, descriptor_1);

    assert!(
        matches!(add_keychain_result, Err(AddKeychainError::<TestKeychain>::KeychainExists{ keychain, descriptor })
                if keychain == TestKeychain::External && descriptor == descriptor_0)
    );
}

// Test error when adding duplicate descriptor with different keychain
#[test]
fn test_add_duplicate_descriptor_error() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(0);

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (descriptor_0, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[0]).unwrap();

    _ = txout_index
        .add_keychain(TestKeychain::External, descriptor_0.clone())
        .expect("no dup keychain or descriptor");

    let add_keychain_result =
        txout_index.add_keychain(TestKeychain::Internal, descriptor_0.clone());

    assert!(
        matches!(add_keychain_result, Err(AddKeychainError::<TestKeychain>::DescriptorExists{ keychain, descriptor })
                if keychain == TestKeychain::External && descriptor == descriptor_0)
    );
}

// test returned changeset when adding new keychains
#[test]
fn test_add_keychains_changeset() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(0);

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (descriptor_0, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[0]).unwrap();
    let (descriptor_1, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[1]).unwrap();
    let (descriptor_2, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[2]).unwrap();

    let change_set = txout_index.add_keychain(TestKeychain::External, descriptor_0.clone());
    let expected_keychains_added = [(TestKeychain::External, descriptor_0)]
        .into_iter()
        .collect::<BTreeMap<TestKeychain, Descriptor<DescriptorPublicKey>>>();
    assert!(matches!(change_set, Ok(ChangeSet::<TestKeychain> {
        keychains_added,
        last_revealed
    }) if keychains_added == expected_keychains_added && last_revealed == Default::default()));

    let change_set = txout_index.add_keychain(TestKeychain::Internal, descriptor_1.clone());
    let expected_keychains_added = [(TestKeychain::Internal, descriptor_1)]
        .into_iter()
        .collect::<BTreeMap<TestKeychain, Descriptor<DescriptorPublicKey>>>();
    assert!(matches!(change_set, Ok(ChangeSet::<TestKeychain> {
        keychains_added,
        last_revealed
    }) if keychains_added == expected_keychains_added && last_revealed == Default::default()));

    let change_set = txout_index.add_keychain(TestKeychain::Other, descriptor_2.clone());
    let expected_keychains_added = [(TestKeychain::Other, descriptor_2)]
        .into_iter()
        .collect::<BTreeMap<TestKeychain, Descriptor<DescriptorPublicKey>>>();
    assert!(matches!(change_set, Ok(ChangeSet::<TestKeychain> {
        keychains_added,
        last_revealed
    }) if keychains_added == expected_keychains_added && last_revealed == Default::default()));
}
