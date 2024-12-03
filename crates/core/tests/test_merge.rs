use std::collections::{BTreeMap, BTreeSet};

use bdk_core::Merge;

#[test]
fn test_btree_map_merge() {
    let mut map1: BTreeMap<i32, &str> = BTreeMap::new();
    map1.insert(1, "a");
    let mut map2: BTreeMap<i32, &str> = BTreeMap::new();
    map2.insert(2, "b");

    map1.merge(map2);

    let expected: BTreeMap<i32, &str> = BTreeMap::from([(1, "a"), (2, "b")]);
    assert_eq!(map1, expected);
}

#[test]
fn test_btree_set_merge() {
    let mut set1: BTreeSet<i32> = BTreeSet::new();
    set1.insert(1);
    let mut set2: BTreeSet<i32> = BTreeSet::new();
    set2.insert(2);

    set1.merge(set2);

    let expected: BTreeSet<i32> = BTreeSet::from([1, 2]);
    assert_eq!(set1, expected);
}

#[test]
fn test_vec_merge() {
    let mut vec1 = vec![1, 2, 3];
    let vec2 = vec![4, 5, 6];

    vec1.merge(vec2);

    assert_eq!(vec1, vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn test_tuple_merge() {
    let mut tuple1 = (vec![1, 2], BTreeSet::from([3]));
    let tuple2 = (vec![3, 4], BTreeSet::from([4]));

    tuple1.merge(tuple2);

    let expected_vec = vec![1, 2, 3, 4];
    assert_eq!(tuple1.0, expected_vec);
    let expected_set: BTreeSet<i32> = BTreeSet::from([3, 4]);
    assert_eq!(tuple1.1, expected_set);
}

#[test]
fn test_is_empty() {
    let map: BTreeMap<i32, i32> = BTreeMap::new();
    assert!(Merge::is_empty(&map));

    let set: BTreeSet<i32> = BTreeSet::new();
    assert!(Merge::is_empty(&set));

    let vec: Vec<i32> = Vec::new();
    assert!(Merge::is_empty(&vec));
}
#[test]
fn test_take() {
    let mut map: BTreeMap<i32, i32> = BTreeMap::new();
    map.insert(1, 1);
    let taken_map = Merge::take(&mut map);
    assert_eq!(taken_map, Some(BTreeMap::from([(1, 1)])));
    assert!(map.is_empty());

    let mut set: BTreeSet<i32> = BTreeSet::new();
    set.insert(1);
    let taken_set = Merge::take(&mut set);
    assert_eq!(taken_set, Some(BTreeSet::from([1])));
    assert!(set.is_empty());

    let mut vec: Vec<i32> = vec![1];
    let taken_vec = Merge::take(&mut vec);
    assert_eq!(taken_vec, Some(vec![1]));
    assert!(vec.is_empty());
}

#[test]
fn test_btree_map_merge_conflict() {
    let mut map1: BTreeMap<i32, &str> = BTreeMap::new();
    map1.insert(1, "a");
    let mut map2: BTreeMap<i32, &str> = BTreeMap::new();
    map2.insert(1, "b");

    map1.merge(map2);

    let expected: BTreeMap<i32, &str> = BTreeMap::from([(1, "b")]);
    assert_eq!(map1, expected);
}

#[test]
fn test_btree_set_merge_conflict() {
    let mut set1: BTreeSet<i32> = BTreeSet::new();
    set1.insert(1);
    let mut set2: BTreeSet<i32> = BTreeSet::new();
    set2.insert(1);

    set1.merge(set2);

    let expected: BTreeSet<i32> = BTreeSet::from([1]);
    assert_eq!(set1, expected);
}

#[test]
fn test_vec_merge_duplicates() {
    let mut vec1 = vec![1, 2, 3];
    let vec2 = vec![3, 4, 5];

    vec1.merge(vec2);

    assert_eq!(vec1, vec![1, 2, 3, 3, 4, 5]);
}
