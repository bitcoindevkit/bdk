use bdk_chain::local_chain::{LocalChain, UpdateNotConnectedError};

#[macro_use]
mod common;

#[test]
fn add_first_tip() {
    let chain = LocalChain::default();
    assert_eq!(
        chain.determine_changeset(&local_chain![(0, h!("A"))]),
        Ok([(0, Some(h!("A")))].into()),
        "add first tip"
    );
}

#[test]
fn add_second_tip() {
    let chain = local_chain![(0, h!("A"))];
    assert_eq!(
        chain.determine_changeset(&local_chain![(0, h!("A")), (1, h!("B"))]),
        Ok([(1, Some(h!("B")))].into())
    );
}

#[test]
fn two_disjoint_chains_cannot_merge() {
    let chain1 = local_chain![(0, h!("A"))];
    let chain2 = local_chain![(1, h!("B"))];
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateNotConnectedError(0))
    );
}

#[test]
fn duplicate_chains_should_merge() {
    let chain1 = local_chain![(0, h!("A"))];
    let chain2 = local_chain![(0, h!("A"))];
    assert_eq!(chain1.determine_changeset(&chain2), Ok(Default::default()));
}

#[test]
fn can_introduce_older_checkpoints() {
    let chain1 = local_chain![(2, h!("C")), (3, h!("D"))];
    let chain2 = local_chain![(1, h!("B")), (2, h!("C"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok([(1, Some(h!("B")))].into())
    );
}

#[test]
fn fix_blockhash_before_agreement_point() {
    let chain1 = local_chain![(0, h!("im-wrong")), (1, h!("we-agree"))];
    let chain2 = local_chain![(0, h!("fix")), (1, h!("we-agree"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok([(0, Some(h!("fix")))].into())
    )
}

/// B and C are in both chain and update
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  |     B   C
/// update | A   B   C   D
/// ```
/// This should succeed with the point of agreement being C and A should be added in addition.
#[test]
fn two_points_of_agreement() {
    let chain1 = local_chain![(1, h!("B")), (2, h!("C"))];
    let chain2 = local_chain![(0, h!("A")), (1, h!("B")), (2, h!("C")), (3, h!("D"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok([(0, Some(h!("A"))), (3, Some(h!("D")))].into()),
    );
}

/// Update and chain does not connect:
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  |     B   C
/// update | A   B       D
/// ```
/// This should fail as we cannot figure out whether C & D are on the same chain
#[test]
fn update_and_chain_does_not_connect() {
    let chain1 = local_chain![(1, h!("B")), (2, h!("C"))];
    let chain2 = local_chain![(0, h!("A")), (1, h!("B")), (3, h!("D"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateNotConnectedError(2)),
    );
}

/// Transient invalidation:
/// ```
///        | 0 | 1 | 2 | 3 | 4 | 5
/// chain  | A       B   C       E
/// update | A       B'  C'  D
/// ```
/// This should succeed and invalidate B,C and E with point of agreement being A.
#[test]
fn transitive_invalidation_applies_to_checkpoints_higher_than_invalidation() {
    let chain1 = local_chain![(0, h!("A")), (2, h!("B")), (3, h!("C")), (5, h!("E"))];
    let chain2 = local_chain![(0, h!("A")), (2, h!("B'")), (3, h!("C'")), (4, h!("D"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok([
            (2, Some(h!("B'"))),
            (3, Some(h!("C'"))),
            (4, Some(h!("D"))),
            (5, None),
        ]
        .into())
    );
}

/// Transient invalidation:
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  |     B   C       E
/// update |     B'  C'  D
/// ```
///
/// This should succeed and invalidate B, C and E with no point of agreement
#[test]
fn transitive_invalidation_applies_to_checkpoints_higher_than_invalidation_no_point_of_agreement() {
    let chain1 = local_chain![(1, h!("B")), (2, h!("C")), (4, h!("E"))];
    let chain2 = local_chain![(1, h!("B'")), (2, h!("C'")), (3, h!("D"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok([
            (1, Some(h!("B'"))),
            (2, Some(h!("C'"))),
            (3, Some(h!("D"))),
            (4, None)
        ]
        .into())
    )
}

/// Transient invalidation:
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  | A   B   C       E
/// update |     B'  C'  D
/// ```
///
/// This should fail since although it tells us that B and C are invalid it doesn't tell us whether
/// A was invalid.
#[test]
fn invalidation_but_no_connection() {
    let chain1 = local_chain![(0, h!("A")), (1, h!("B")), (2, h!("C")), (4, h!("E"))];
    let chain2 = local_chain![(1, h!("B'")), (2, h!("C'")), (3, h!("D"))];

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateNotConnectedError(0))
    )
}
