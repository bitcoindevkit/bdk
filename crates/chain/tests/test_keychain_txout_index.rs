#![cfg(feature = "miniscript")]

use bdk_chain::{
    collections::BTreeMap,
    indexer::keychain_txout::{ChangeSet, KeychainTxOutIndex},
    DescriptorExt, DescriptorId, Indexer, Merge,
};
use bdk_testenv::{
    hash,
    utils::{new_tx, DESCRIPTORS},
};
use bitcoin::{secp256k1::Secp256k1, Amount, OutPoint, ScriptBuf, Transaction, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

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
) -> KeychainTxOutIndex<TestKeychain> {
    let mut txout_index = KeychainTxOutIndex::<TestKeychain>::new(lookahead);

    let _ = txout_index
        .insert_descriptor(TestKeychain::External, external_descriptor)
        .unwrap();
    let _ = txout_index
        .insert_descriptor(TestKeychain::Internal, internal_descriptor)
        .unwrap();

    txout_index
}

fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

// We create two empty changesets lhs and rhs, we then insert various descriptors with various
// last_revealed, merge rhs to lhs, and check that the result is consistent with these rules:
// - Existing index doesn't update if the new index in `other` is lower than `self`.
// - Existing index updates if the new index in `other` is higher than `self`.
// - Existing index is unchanged if keychain doesn't exist in `other`.
// - New keychain gets added if the keychain is in `other` but not in `self`.
#[test]
fn merge_changesets_check_last_revealed() {
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
        last_revealed: lhs_di,
    };
    let rhs = ChangeSet {
        last_revealed: rhs_di,
    };
    lhs.merge(rhs);

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
fn test_set_all_derivation_indices() {
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
        txout_index.reveal_to_target_multi(&derive_to),
        ChangeSet {
            last_revealed: last_revealed.clone()
        }
    );
    assert_eq!(txout_index.last_revealed_indices(), derive_to);
    assert_eq!(
        txout_index.reveal_to_target_multi(&derive_to),
        ChangeSet::default(),
        "no changes if we set to the same thing"
    );
    assert_eq!(txout_index.initial_changeset().last_revealed, last_revealed);
}

#[test]
fn test_lookahead() {
    let external_descriptor = parse_descriptor(DESCRIPTORS[0]);
    let internal_descriptor = parse_descriptor(DESCRIPTORS[1]);
    let lookahead = 10;
    let mut txout_index = init_txout_index(
        external_descriptor.clone(),
        internal_descriptor.clone(),
        lookahead,
    );

    // given:
    // - external lookahead set to 10
    // when:
    // - set external derivation index to value higher than last, but within the lookahead value
    // expect:
    // - scripts cached in spk_txout_index should increase correctly
    // - stored scripts of external keychain should be of expected counts
    for index in 0..20 {
        let (revealed_spks, revealed_changeset) = txout_index
            .reveal_to_target(TestKeychain::External, index)
            .unwrap();
        assert_eq!(
            revealed_spks,
            vec![(index, spk_at_index(&external_descriptor, index))],
        );
        assert_eq!(
            &revealed_changeset.last_revealed,
            &[(external_descriptor.descriptor_id(), index)].into()
        );

        // test stored spks are expected
        let exp_last_store_index = index + lookahead;
        for i in index + 1..=exp_last_store_index {
            assert_eq!(
                txout_index.spk_at_index(TestKeychain::External, i),
                Some(spk_at_index(&external_descriptor, i))
            );
        }
        assert!(txout_index
            .spk_at_index(TestKeychain::External, exp_last_store_index + 1)
            .is_none());

        // internal should only have lookahead
        for i in 0..lookahead {
            assert_eq!(
                txout_index.spk_at_index(TestKeychain::Internal, i),
                Some(spk_at_index(&internal_descriptor, i))
            );
        }
        assert!(txout_index
            .spk_at_index(TestKeychain::Internal, lookahead)
            .is_none());

        assert_eq!(
            txout_index
                .revealed_keychain_spks(TestKeychain::External)
                .count(),
            index as usize + 1,
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(TestKeychain::Internal)
                .count(),
            0,
        );
        assert_eq!(
            txout_index
                .unused_keychain_spks(TestKeychain::External)
                .count(),
            index as usize + 1,
        );
        assert_eq!(
            txout_index
                .unused_keychain_spks(TestKeychain::Internal)
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
    let reveal_to = 24;
    let (revealed_spks, revealed_changeset) = txout_index
        .reveal_to_target(TestKeychain::Internal, reveal_to)
        .unwrap();
    assert_eq!(
        revealed_spks,
        (0..=reveal_to)
            .map(|index| (index, spk_at_index(&internal_descriptor, index)))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        &revealed_changeset.last_revealed,
        &[(internal_descriptor.descriptor_id(), reveal_to)].into()
    );

    // test stored spks are expected
    let exp_last_store_index = reveal_to + lookahead;
    for index in reveal_to + 1..=exp_last_store_index {
        assert_eq!(
            txout_index.spk_at_index(TestKeychain::Internal, index),
            Some(spk_at_index(&internal_descriptor, index))
        );
    }
    assert!(txout_index
        .spk_at_index(TestKeychain::Internal, exp_last_store_index + 1)
        .is_none());

    assert_eq!(
        txout_index
            .revealed_keychain_spks(TestKeychain::Internal)
            .count(),
        25,
    );

    // ensure derivation indices are expected for each keychain
    let last_external_index = txout_index
        .last_revealed_index(TestKeychain::External)
        .expect("already derived");
    let last_internal_index = txout_index
        .last_revealed_index(TestKeychain::Internal)
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
            ..new_tx(external_index)
        };
        assert_eq!(txout_index.index_tx(&tx), ChangeSet::default());
        assert_eq!(
            txout_index.last_revealed_index(TestKeychain::External),
            Some(last_external_index)
        );
        assert_eq!(
            txout_index.last_revealed_index(TestKeychain::Internal),
            Some(last_internal_index)
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(TestKeychain::External)
                .count(),
            last_external_index as usize + 1,
        );
        assert_eq!(
            txout_index
                .revealed_keychain_spks(TestKeychain::Internal)
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
        let op = OutPoint::new(hash!("fake tx"), spk_i);
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
            txout_index.last_revealed_index(TestKeychain::External),
            Some(spk_i)
        );
        assert_eq!(
            txout_index.last_used_index(TestKeychain::External),
            Some(spk_i)
        );
    }

    // now try with index 41 (lookahead surpassed), we expect that the txout to not be indexed
    let spk_41 = external_descriptor
        .at_derivation_index(41)
        .unwrap()
        .script_pubkey();
    let op = OutPoint::new(hash!("fake tx"), 41);
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
    assert_eq!(txout_index.next_index(TestKeychain::External).unwrap(), (0, true));
    let (spk, changeset) = txout_index.reveal_next_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (0_u32, external_spk_0.clone()));
    assert_eq!(&changeset.last_revealed, &[(external_descriptor.descriptor_id(), 0)].into());
    let (spk, changeset) = txout_index.next_unused_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (0_u32, external_spk_0.clone()));
    assert_eq!(&changeset.last_revealed, &[].into());

    // - derived till 25
    // - used all spks till 15.
    // - used list : [0..=15, 17, 20, 23]
    // - unused list: [16, 18, 19, 21, 22, 24, 25]

    // - next_derivation_index() = (26, true)
    // - derive_new() = ((26, <spk>), keychain::ChangeSet)
    // - next_unused() == ((16, <spk>), keychain::ChangeSet::is_empty())
    let _ = txout_index.reveal_to_target(TestKeychain::External, 25);

    (0..=15)
        .chain([17, 20, 23])
        .for_each(|index| assert!(txout_index.mark_used(TestKeychain::External, index)));

    assert_eq!(txout_index.next_index(TestKeychain::External).unwrap(), (26, true));

    let (spk, changeset) = txout_index.reveal_next_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (26, external_spk_26));

    assert_eq!(&changeset.last_revealed, &[(external_descriptor.descriptor_id(), 26)].into());

    let (spk, changeset) = txout_index.next_unused_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (16, external_spk_16));
    assert_eq!(&changeset.last_revealed, &[].into());

    // - Use all the derived till 26.
    // - next_unused() = ((27, <spk>), keychain::ChangeSet)
    (0..=26).for_each(|index| {
        txout_index.mark_used(TestKeychain::External, index);
    });

    let (spk, changeset) = txout_index.next_unused_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (27, external_spk_27));
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

    let _ = txout_index
        .insert_descriptor(TestKeychain::External, no_wildcard_descriptor.clone())
        .unwrap();

    // given:
    // - `txout_index` with no stored scripts
    // expect:
    // - next derivation index should be new
    // - when we derive a new script, script @ index 0
    // - when we get the next unused script, script @ index 0
    assert_eq!(
        txout_index.next_index(TestKeychain::External).unwrap(),
        (0, true)
    );
    let (spk, changeset) = txout_index.reveal_next_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (0, external_spk.clone()));
    assert_eq!(
        &changeset.last_revealed,
        &[(no_wildcard_descriptor.descriptor_id(), 0)].into()
    );

    let (spk, changeset) = txout_index.next_unused_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (0, external_spk.clone()));
    assert_eq!(&changeset.last_revealed, &[].into());

    // given:
    // - the non-wildcard descriptor already has a stored and used script
    // expect:
    // - next derivation index should not be new
    // - derive new and next unused should return the old script
    // - store_up_to should not panic and return empty changeset
    assert_eq!(
        txout_index.next_index(TestKeychain::External).unwrap(),
        (0, false)
    );
    txout_index.mark_used(TestKeychain::External, 0);

    let (spk, changeset) = txout_index.reveal_next_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (0, external_spk.clone()));
    assert_eq!(&changeset.last_revealed, &[].into());

    let (spk, changeset) = txout_index.next_unused_spk(TestKeychain::External).unwrap();
    assert_eq!(spk, (0, external_spk.clone()));
    assert_eq!(&changeset.last_revealed, &[].into());
    let (revealed_spks, revealed_changeset) = txout_index
        .reveal_to_target(TestKeychain::External, 200)
        .unwrap();
    assert_eq!(revealed_spks.len(), 0);
    assert!(revealed_changeset.is_empty());

    // we check that spks_of_keychain returns a SpkIterator with just one element
    assert_eq!(
        txout_index
            .revealed_keychain_spks(TestKeychain::External)
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
            let _ = index.reveal_to_target(TestKeychain::External, last_revealed);
        }
        if let Some(last_revealed) = t.internal_last_revealed {
            let _ = index.reveal_to_target(TestKeychain::Internal, last_revealed);
        }

        let keychain_test_cases = [
            (
                TestKeychain::External,
                t.external_last_revealed,
                t.external_target,
            ),
            (
                TestKeychain::Internal,
                t.internal_last_revealed,
                t.internal_target,
            ),
        ];
        for (keychain, last_revealed, target) in keychain_test_cases {
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
                index.lookahead_to_target(keychain.clone(), target);
                let keys: Vec<_> = (0..)
                    .take_while(|&i| index.spk_at_index(keychain.clone(), i).is_some())
                    .collect();
                let exp_keys: Vec<_> = (0..=exp_last_stored_index).collect();
                assert_eq!(keys, exp_keys);
            }
        }
    }
}

#[test]
fn insert_descriptor_should_reject_hardened_steps() {
    use bdk_chain::keychain_txout::KeychainTxOutIndex;

    // This descriptor has hardened child steps
    let s = "wpkh([e273fe42/84h/1h/0h/0h]tpubDEBY7DLZ5kK6pMC2qdtVvKpe6CiAmVDx1SiLtLS3V4GAGyRvsuVbCYReJ9oQz1rfMapghJyUAYLqkqJ3Xadp3GSTCtdAFcKPgzAXC1hzz8a/*h)";
    let (desc, _) =
        <Descriptor<DescriptorPublicKey>>::parse_descriptor(&Secp256k1::new(), s).unwrap();

    let mut indexer = KeychainTxOutIndex::<&str>::new(10);
    assert!(indexer.insert_descriptor("keychain", desc).is_err())
}

#[test]
fn applying_changesets_one_by_one_vs_aggregate_must_have_same_result() {
    let desc = parse_descriptor(DESCRIPTORS[0]);
    let changesets: &[ChangeSet] = &[
        ChangeSet {
            last_revealed: [(desc.descriptor_id(), 10)].into(),
        },
        ChangeSet {
            last_revealed: [(desc.descriptor_id(), 12)].into(),
        },
    ];

    let mut indexer_a = KeychainTxOutIndex::<TestKeychain>::new(0);
    indexer_a
        .insert_descriptor(TestKeychain::External, desc.clone())
        .expect("must insert keychain");
    for changeset in changesets {
        indexer_a.apply_changeset(changeset.clone());
    }

    let mut indexer_b = KeychainTxOutIndex::<TestKeychain>::new(0);
    indexer_b
        .insert_descriptor(TestKeychain::External, desc.clone())
        .expect("must insert keychain");
    let aggregate_changesets = changesets
        .iter()
        .cloned()
        .reduce(|mut agg, cs| {
            agg.merge(cs);
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

#[test]
fn assigning_same_descriptor_to_multiple_keychains_should_error() {
    let desc = parse_descriptor(DESCRIPTORS[0]);
    let mut indexer = KeychainTxOutIndex::<TestKeychain>::new(0);
    let _ = indexer
        .insert_descriptor(TestKeychain::Internal, desc.clone())
        .unwrap();
    assert!(indexer
        .insert_descriptor(TestKeychain::External, desc)
        .is_err())
}

#[test]
fn reassigning_keychain_to_a_new_descriptor_should_error() {
    let desc1 = parse_descriptor(DESCRIPTORS[0]);
    let desc2 = parse_descriptor(DESCRIPTORS[1]);
    let mut indexer = KeychainTxOutIndex::<TestKeychain>::new(0);
    let _ = indexer.insert_descriptor(TestKeychain::Internal, desc1);
    assert!(indexer
        .insert_descriptor(TestKeychain::Internal, desc2)
        .is_err());
}

#[test]
fn when_querying_over_a_range_of_keychains_the_utxos_should_show_up() {
    let mut indexer = KeychainTxOutIndex::<usize>::new(0);
    let mut tx = new_tx(0);

    for (i, descriptor) in DESCRIPTORS.iter().enumerate() {
        let descriptor = parse_descriptor(descriptor);
        let _ = indexer.insert_descriptor(i, descriptor.clone()).unwrap();
        if i != 4 {
            // skip one in the middle to see if uncovers any bugs
            indexer.reveal_next_spk(i);
        }
        tx.output.push(TxOut {
            script_pubkey: descriptor.at_derivation_index(0).unwrap().script_pubkey(),
            value: Amount::from_sat(10_000),
        });
    }

    let n_spks = DESCRIPTORS.len() - /*we skipped one*/ 1;

    let _ = indexer.index_tx(&tx);
    assert_eq!(indexer.outpoints().len(), n_spks);

    assert_eq!(indexer.revealed_spks(0..DESCRIPTORS.len()).count(), n_spks);
    assert_eq!(indexer.revealed_spks(1..4).count(), 4 - 1);
    assert_eq!(
        indexer.net_value(&tx, 0..DESCRIPTORS.len()).to_sat(),
        (10_000 * n_spks) as i64
    );
    assert_eq!(
        indexer.net_value(&tx, 3..6).to_sat(),
        (10_000 * (6 - 3 - /*the skipped one*/ 1)) as i64
    );
}
