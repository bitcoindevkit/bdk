#![cfg(feature = "miniscript")]

use bdk_chain::local_chain::{ChangeSet, LocalChain};
use bdk_testenv::{hash, local_chain};
use bitcoin::BlockHash;

#[test]
fn apply_changeset_does_not_replace_genesis() {
    let mut chain: LocalChain = local_chain![(0, hash!("G")), (1, hash!("A"))];
    let changeset: ChangeSet<BlockHash> = [(0, Some(hash!("not_G")))].into_iter().collect();
    let result = chain.apply_changeset(&changeset);
    assert!(result.is_err());
    assert_eq!(chain.genesis_hash(), hash!("G"));
    assert_eq!(chain.tip().hash(), hash!("A")); // tip unchanged too
}

#[test]
fn apply_changeset_with_matching_genesis_extends_chain() {
    let mut chain: LocalChain = local_chain![(0, hash!("G")), (1, hash!("A"))];
    let changeset: ChangeSet<BlockHash> = [(0, Some(hash!("G"))), (2, Some(hash!("B")))]
        .into_iter()
        .collect();
    chain
        .apply_changeset(&changeset)
        .expect("matching genesis must apply");
    assert_eq!(chain.genesis_hash(), hash!("G"));
    assert_eq!(chain.tip().hash(), hash!("B"));
    assert_eq!(chain.get(2).map(|cp| cp.hash()), Some(hash!("B")));
}

#[test]
fn apply_changeset_above_genesis_still_applies() {
    let mut chain: LocalChain = local_chain![(0, hash!("G")), (1, hash!("A"))];
    let changeset: ChangeSet<BlockHash> = [(2, Some(hash!("B")))].into_iter().collect();
    chain
        .apply_changeset(&changeset)
        .expect("changeset above genesis must apply");
    assert_eq!(chain.genesis_hash(), hash!("G"));
    assert_eq!(chain.tip().hash(), hash!("B"));
}
