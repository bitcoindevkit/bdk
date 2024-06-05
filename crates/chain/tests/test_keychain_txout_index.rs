#![cfg(feature = "miniscript")]

#[macro_use]
mod common;
use bdk_chain::{
    collections::BTreeMap,
    indexed_tx_graph::Indexer,
    keychain::{self, ChangeSet, KeychainTxOutIndex},
    Append, DescriptorExt, DescriptorId,
};

use bitcoin::{secp256k1::Secp256k1, Amount, OutPoint, ScriptBuf, Transaction, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::common::DESCRIPTORS;

#[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
enum TestKeychain {
    External,
    Internal,
}

fn parse_descriptor(descriptor: &str) -> Descriptor<DescriptorPublicKey> {
    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, descriptor)
        .unwrap()
        .0
}

fn init_txout_index(
    external_descriptor: Descriptor<DescriptorPublicKey>,
    internal_descriptor: Descriptor<DescriptorPublicKey>,
    lookahead: u32,
) -> bdk_chain::keychain::KeychainTxOutIndex<TestKeychain> {
    let mut txout_index = bdk_chain::keychain::KeychainTxOutIndex::<TestKeychain>::new(lookahead);

    let _ = txout_index.insert_descriptor(TestKeychain::External, external_descriptor);
    let _ = txout_index.insert_descriptor(TestKeychain::Internal, internal_descriptor);

    txout_index
}

fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

// We create two empty changesets lhs and rhs, we then insert various descriptors with various
// last_revealed, append rhs to lhs, and check that the result is consistent with these rules:
// - Existing index doesn't update if the new index in `other` is lower than `self`.
// - Existing index updates if the new index in `other` is higher than `self`.
// - Existing index is unchanged if keychain doesn't exist in `other`.
// - New keychain gets added if the keychain is in `other` but not in `self`.
#[test]
fn append_changesets_check_last_revealed() {
    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let descriptor_ids: Vec<_> = DESCRIPTORS
        .iter()
        .take(4)
        .map(|d| {
            Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, d)
                .unwrap()
                .0
                .descriptor_id()
        })
        .collect();

    let mut lhs_di = BTreeMap::<DescriptorId, u32>::default();
    let mut rhs_di = BTreeMap::<DescriptorId, u32>::default();
    lhs_di.insert(descriptor_ids[0], 7);
    lhs_di.insert(descriptor_ids[1], 0);
    lhs_di.insert(descriptor_ids[2], 3);

    rhs_di.insert(descriptor_ids[0], 3); // value less than lhs desc 0
    rhs_di.insert(descriptor_ids[1], 5); // value more than lhs desc 1
    lhs_di.insert(descriptor_ids[3], 4); // key doesn't exist in lhs

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
    assert_eq!(lhs.last_revealed.get(&descriptor_ids[0]), Some(&7));
    // Existing index updates if the new index in `other` is higher than `self`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids[1]), Some(&5));
    // Existing index is unchanged if keychain doesn't exist in `other`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids[2]), Some(&3));
    // New keychain gets added if the keychain is in `other` but not in `self`.
    assert_eq!(lhs.last_revealed.get(&descriptor_ids[3]), Some(&4));
}

#[test]
fn test_apply_changeset_with_different_descriptors_to_same_keychain() {
    let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
    let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
    let mut txout_index =
        init_txout_index(external_descriptor.clone(), internal_descriptor.clone(), 0);
    assert_eq!(
        txout_index.keychains().collect::<Vec<_>>(),
        vec![
            (&TestKeychain::External, &external_descriptor),
            (&TestKeychain::Internal, &internal_descriptor)
        ]
    );

    let changeset = ChangeSet {
        keychains_added: [(TestKeychain::External, internal_descriptor.clone())].into(),
        last_revealed: [].into(),
    };
    txout_index.apply_changeset(changeset);

    assert_eq!(
        txout_index.keychains().collect::<Vec<_>>(),
        vec![
            (&TestKeychain::External, &internal_descriptor),
            (&TestKeychain::Internal, &internal_descriptor)
        ]
    );

    let changeset = ChangeSet {
        keychains_added: [(TestKeychain::Internal, external_descriptor.clone())].into(),
        last_revealed: [].into(),
    };
    txout_index.apply_changeset(changeset);

    assert_eq!(
        txout_index.keychains().collect::<Vec<_>>(),
        vec![
            (&TestKeychain::External, &internal_descriptor),
            (&TestKeychain::Internal, &external_descriptor)
        ]
    );
}

#[test]
fn test_set_all_derivation_indices() {
    use bdk_chain::indexed_tx_graph::Indexer;

    let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
    let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
    let mut txout_index =
        init_txout_index(external_descriptor.clone(), internal_descriptor.clone(), 0);
    let derive_to: BTreeMap<_, _> =
        [(TestKeychain::External, 12), (TestKeychain::Internal, 24)].into();
    let last_revealed: BTreeMap<_, _> = [
        (external_descriptor.descriptor_id(), 12),
        (internal_descriptor.descriptor_id(), 24),
    ]
    .into();
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
    let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
    let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
    let mut txout_index =
        init_txout_index(external_descriptor.clone(), internal_descriptor.clone(), 10);

    // given:
    // - external lookahead set to 10
    // when:
    // - set external derivation index to value higher than last, but within the lookahead value
    // expect:
    // - scripts cached in spk_txout_index should increase correctly
    // - stored scripts of external keychain should be of expected counts
    for index in (0..20).skip_while(|i| i % 2 == 1) {
        let (revealed_spks, revealed_changeset) = txout_index
            .reveal_to_target(&TestKeychain::External, index)
            .unwrap();
        assert_eq!(
            revealed_spks.collect::<Vec<_>>(),
            vec![(index, spk_at_index(&external_descriptor, index))],
        );
        assert_eq!(
            &revealed_changeset.last_revealed,
            &[(external_descriptor.descriptor_id(), index)].into()
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
    let (revealed_spks, revealed_changeset) = txout_index
        .reveal_to_target(&TestKeychain::Internal, 24)
        .unwrap();
    assert_eq!(
        revealed_spks.collect::<Vec<_>>(),
        (0..=24)
            .map(|index| (index, spk_at_index(&internal_descriptor, index)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        &revealed_changeset.last_revealed,
        &[(internal_descriptor.descriptor_id(), 24)].into()
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
                    script_pubkey: external_descriptor
                        .at_derivation_index(external_index)
                        .unwrap()
                        .script_pubkey(),
                    value: Amount::from_sat(10_000),
                },
                TxOut {
                    script_pubkey: internal_descriptor
                        .at_derivation_index(internal_index)
                        .unwrap()
                        .script_pubkey(),
                    value: Amount::from_sat(10_000),
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
    let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
    let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
    let mut txout_index =
        init_txout_index(external_descriptor.clone(), internal_descriptor.clone(), 10);

    let spks: BTreeMap<u32, ScriptBuf> = [0, 10, 20, 30]
        .into_iter()
        .map(|i| {
            (
                i,
                external_descriptor
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
            value: Amount::ZERO,
        };

        let changeset = txout_index.index_txout(op, &txout);
        assert_eq!(
            &changeset.last_revealed,
            &[(external_descriptor.descriptor_id(), spk_i)].into()
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
    let spk_41 = external_descriptor
        .at_derivation_index(41)
        .unwrap()
        .script_pubkey();
    let op = OutPoint::new(h!("fake tx"), 41);
    let txout = TxOut {
        script_pubkey: spk_41,
        value: Amount::ZERO,
    };
    let changeset = txout_index.index_txout(op, &txout);
    assert!(changeset.is_empty());
}

#[test]
#[rustfmt::skip]
fn test_wildcard_derivations() {
    let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
    let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
    let mut txout_index = init_txout_index(external_descriptor.clone(), internal_descriptor.clone(), 0);
    let external_spk_0 = external_descriptor.at_derivation_index(0).unwrap().script_pubkey();
    let external_spk_16 = external_descriptor.at_derivation_index(16).unwrap().script_pubkey();
    let external_spk_26 = external_descriptor.at_derivation_index(26).unwrap().script_pubkey();
    let external_spk_27 = external_descriptor.at_derivation_index(27).unwrap().script_pubkey();

    // - nothing is derived
    // - unused list is also empty
    //
    // - next_derivation_index() == (0, true)
    // - derive_new() == ((0, <spk>), keychain::ChangeSet)
    // - next_unused() == ((0, <spk>), keychain::ChangeSet:is_empty())
    assert_eq!(txout_index.next_index(&TestKeychain::External).unwrap(), (0, true));
    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External).unwrap();
    assert_eq!(spk, (0_u32, external_spk_0.as_script()));
    assert_eq!(&changeset.last_revealed, &[(external_descriptor.descriptor_id(), 0)].into());
    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External).unwrap();
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

    assert_eq!(txout_index.next_index(&TestKeychain::External).unwrap(), (26, true));

    let (spk, changeset) = txout_index.reveal_next_spk(&TestKeychain::External).unwrap();
    assert_eq!(spk, (26, external_spk_26.as_script()));

    assert_eq!(&changeset.last_revealed, &[(external_descriptor.descriptor_id(), 26)].into());

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External).unwrap();
    assert_eq!(spk, (16, external_spk_16.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());

    // - Use all the derived till 26.
    // - next_unused() = ((27, <spk>), keychain::ChangeSet)
    (0..=26).for_each(|index| {
        txout_index.mark_used(TestKeychain::External, index);
    });

    let (spk, changeset) = txout_index.next_unused_spk(&TestKeychain::External).unwrap();
    assert_eq!(spk, (27, external_spk_27.as_script()));
    assert_eq!(&changeset.last_revealed, &[(external_descriptor.descriptor_id(), 27)].into());
}

#[test]
fn test_non_wildcard_derivations() {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(0);

    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    let (no_wildcard_descriptor, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[6]).unwrap();
    let external_spk = no_wildcard_descriptor
        .at_derivation_index(0)
        .unwrap()
        .script_pubkey();

    let _ = txout_index.insert_descriptor(TestKeychain::External, no_wildcard_descriptor.clone());

    // given:
    // - `txout_index` with no stored scripts
    // expect:
    // - next derivation index should be new
    // - when we derive a new script, script @ index 0
    // - when we get the next unused script, script @ index 0
    assert_eq!(
        txout_index.next_index(&TestKeychain::External).unwrap(),
        (0, true)
    );
    let (spk, changeset) = txout_index
        .reveal_next_spk(&TestKeychain::External)
        .unwrap();
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(
        &changeset.last_revealed,
        &[(no_wildcard_descriptor.descriptor_id(), 0)].into()
    );

    let (spk, changeset) = txout_index
        .next_unused_spk(&TestKeychain::External)
        .unwrap();
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());

    // given:
    // - the non-wildcard descriptor already has a stored and used script
    // expect:
    // - next derivation index should not be new
    // - derive new and next unused should return the old script
    // - store_up_to should not panic and return empty changeset
    assert_eq!(
        txout_index.next_index(&TestKeychain::External).unwrap(),
        (0, false)
    );
    txout_index.mark_used(TestKeychain::External, 0);

    let (spk, changeset) = txout_index
        .reveal_next_spk(&TestKeychain::External)
        .unwrap();
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());

    let (spk, changeset) = txout_index
        .next_unused_spk(&TestKeychain::External)
        .unwrap();
    assert_eq!(spk, (0, external_spk.as_script()));
    assert_eq!(&changeset.last_revealed, &[].into());
    let (revealed_spks, revealed_changeset) = txout_index
        .reveal_to_target(&TestKeychain::External, 200)
        .unwrap();
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

/// Check that calling `lookahead_to_target` stores the expected spks.
#[test]
fn lookahead_to_target() {
    #[derive(Default)]
    struct TestCase {
        /// Global lookahead value.
        lookahead: u32,
        /// Last revealed index for external keychain.
        external_last_revealed: Option<u32>,
        /// Last revealed index for internal keychain.
        internal_last_revealed: Option<u32>,
        /// Call `lookahead_to_target(External, u32)`.
        external_target: Option<u32>,
        /// Call `lookahead_to_target(Internal, u32)`.
        internal_target: Option<u32>,
    }

    let test_cases = &[
        TestCase {
            lookahead: 0,
            external_target: Some(100),
            ..Default::default()
        },
        TestCase {
            lookahead: 10,
            internal_target: Some(99),
            ..Default::default()
        },
        TestCase {
            lookahead: 100,
            internal_target: Some(9),
            external_target: Some(10),
            ..Default::default()
        },
        TestCase {
            lookahead: 12,
            external_last_revealed: Some(2),
            internal_last_revealed: Some(2),
            internal_target: Some(15),
            external_target: Some(13),
        },
        TestCase {
            lookahead: 13,
            external_last_revealed: Some(100),
            internal_last_revealed: Some(21),
            internal_target: Some(120),
            external_target: Some(130),
        },
    ];

    for t in test_cases {
        let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
        let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
        let mut index = init_txout_index(
            external_descriptor.clone(),
            internal_descriptor.clone(),
            t.lookahead,
        );

        if let Some(last_revealed) = t.external_last_revealed {
            let _ = index.reveal_to_target(&TestKeychain::External, last_revealed);
        }
        if let Some(last_revealed) = t.internal_last_revealed {
            let _ = index.reveal_to_target(&TestKeychain::Internal, last_revealed);
        }

        let keychain_test_cases = [
            (
                external_descriptor.descriptor_id(),
                TestKeychain::External,
                t.external_last_revealed,
                t.external_target,
            ),
            (
                internal_descriptor.descriptor_id(),
                TestKeychain::Internal,
                t.internal_last_revealed,
                t.internal_target,
            ),
        ];
        for (descriptor_id, keychain, last_revealed, target) in keychain_test_cases {
            if let Some(target) = target {
                let original_last_stored_index = match last_revealed {
                    Some(last_revealed) => Some(last_revealed + t.lookahead),
                    None => t.lookahead.checked_sub(1),
                };
                let exp_last_stored_index = match original_last_stored_index {
                    Some(original_last_stored_index) => {
                        Ord::max(target, original_last_stored_index)
                    }
                    None => target,
                };
                index.lookahead_to_target(&keychain, target);
                let keys = index
                    .inner()
                    .all_spks()
                    .range((descriptor_id, 0)..=(descriptor_id, u32::MAX))
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>();
                let exp_keys = core::iter::repeat(descriptor_id)
                    .zip(0_u32..=exp_last_stored_index)
                    .collect::<Vec<_>>();
                assert_eq!(keys, exp_keys);
            }
        }
    }
}

/// `::index_txout` should still index txouts with spks derived from descriptors without keychains.
/// This includes properly refilling the lookahead for said descriptors.
#[test]
fn index_txout_after_changing_descriptor_under_keychain() {
    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    let (desc_a, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[0])
        .expect("descriptor 0 must be valid");
    let (desc_b, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[1])
        .expect("descriptor 1 must be valid");
    let desc_id_a = desc_a.descriptor_id();

    let mut txout_index = bdk_chain::keychain::KeychainTxOutIndex::<()>::new(10);

    // Introduce `desc_a` under keychain `()` and replace the descriptor.
    let _ = txout_index.insert_descriptor((), desc_a.clone());
    let _ = txout_index.insert_descriptor((), desc_b.clone());

    // Loop through spks in intervals of `lookahead` to create outputs with. We should always be
    // able to index these outputs if `lookahead` is respected.
    let spk_indices = [9, 19, 29, 39];
    for i in spk_indices {
        let spk_at_index = desc_a
            .at_derivation_index(i)
            .expect("must derive")
            .script_pubkey();
        let index_changeset = txout_index.index_txout(
            // Use spk derivation index as vout as we just want an unique outpoint.
            OutPoint::new(h!("mock_tx"), i as _),
            &TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: spk_at_index,
            },
        );
        assert_eq!(
            index_changeset,
            bdk_chain::keychain::ChangeSet {
                keychains_added: BTreeMap::default(),
                last_revealed: [(desc_id_a, i)].into(),
            },
            "must always increase last active if impl respects lookahead"
        );
    }
}

#[test]
fn insert_descriptor_no_change() {
    let secp = Secp256k1::signing_only();
    let (desc, _) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, DESCRIPTORS[0]).unwrap();
    let mut txout_index = KeychainTxOutIndex::<()>::default();
    assert_eq!(
        txout_index.insert_descriptor((), desc.clone()),
        keychain::ChangeSet {
            keychains_added: [((), desc.clone())].into(),
            last_revealed: Default::default()
        },
    );
    assert_eq!(
        txout_index.insert_descriptor((), desc.clone()),
        keychain::ChangeSet::default(),
        "inserting the same descriptor for keychain should return an empty changeset",
    );
}

#[test]
fn applying_changesets_one_by_one_vs_aggregate_must_have_same_result() {
    let desc = parse_descriptor(DESCRIPTORS[0]);
    let changesets: &[ChangeSet<TestKeychain>] = &[
        ChangeSet {
            keychains_added: [(TestKeychain::Internal, desc.clone())].into(),
            last_revealed: [].into(),
        },
        ChangeSet {
            keychains_added: [(TestKeychain::External, desc.clone())].into(),
            last_revealed: [(desc.descriptor_id(), 12)].into(),
        },
    ];

    let mut indexer_a = KeychainTxOutIndex::<TestKeychain>::new(0);
    for changeset in changesets {
        indexer_a.apply_changeset(changeset.clone());
    }

    let mut indexer_b = KeychainTxOutIndex::<TestKeychain>::new(0);
    let aggregate_changesets = changesets
        .iter()
        .cloned()
        .reduce(|mut agg, cs| {
            agg.append(cs);
            agg
        })
        .expect("must aggregate changesets");
    indexer_b.apply_changeset(aggregate_changesets);

    assert_eq!(
        indexer_a.keychains().collect::<Vec<_>>(),
        indexer_b.keychains().collect::<Vec<_>>()
    );
    assert_eq!(
        indexer_a.spk_at_index(TestKeychain::External, 0),
        indexer_b.spk_at_index(TestKeychain::External, 0)
    );
    assert_eq!(
        indexer_a.spk_at_index(TestKeychain::Internal, 0),
        indexer_b.spk_at_index(TestKeychain::Internal, 0)
    );
    assert_eq!(
        indexer_a.last_revealed_indices(),
        indexer_b.last_revealed_indices()
    );
}

// When the same descriptor is associated with various keychains,
// index methods only return the highest keychain by Ord
#[test]
fn test_only_highest_ord_keychain_is_returned() {
    let desc = parse_descriptor(DESCRIPTORS[0]);

    let mut indexer = KeychainTxOutIndex::<TestKeychain>::new(0);
    let _ = indexer.insert_descriptor(TestKeychain::Internal, desc.clone());
    let _ = indexer.insert_descriptor(TestKeychain::External, desc);

    // reveal_next_spk will work with either keychain
    let spk0: ScriptBuf = indexer
        .reveal_next_spk(&TestKeychain::External)
        .unwrap()
        .0
         .1
        .into();
    let spk1: ScriptBuf = indexer
        .reveal_next_spk(&TestKeychain::Internal)
        .unwrap()
        .0
         .1
        .into();

    // index_of_spk will always return External
    assert_eq!(
        indexer.index_of_spk(&spk0),
        Some((TestKeychain::External, 0))
    );
    assert_eq!(
        indexer.index_of_spk(&spk1),
        Some((TestKeychain::External, 1))
    );
}

#[test]
fn when_querying_over_a_range_of_keychains_the_utxos_should_show_up() {
    let mut indexer = KeychainTxOutIndex::<usize>::new(0);
    let mut tx = common::new_tx(0);

    for (i, descriptor) in DESCRIPTORS.iter().enumerate() {
        let descriptor = parse_descriptor(descriptor);
        let _ = indexer.insert_descriptor(i, descriptor.clone());
        indexer.reveal_next_spk(&i);
        tx.output.push(TxOut {
            script_pubkey: descriptor.at_derivation_index(0).unwrap().script_pubkey(),
            value: Amount::from_sat(10_000),
        });
    }

    let _ = indexer.index_tx(&tx);
    assert_eq!(indexer.outpoints().count(), DESCRIPTORS.len());

    assert_eq!(
        indexer.revealed_spks(0..DESCRIPTORS.len()).count(),
        DESCRIPTORS.len()
    );
    assert_eq!(indexer.revealed_spks(1..4).count(), 4 - 1);
    assert_eq!(
        indexer.net_value(&tx, 0..DESCRIPTORS.len()).to_sat(),
        (10_000 * DESCRIPTORS.len()) as i64
    );
    assert_eq!(
        indexer.net_value(&tx, 3..5).to_sat(),
        (10_000 * (5 - 3)) as i64
    );
}
