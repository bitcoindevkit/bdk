#![allow(dead_code)]

use bdk_core::bitcoin::key::{Secp256k1, UntweakedPublicKey};
use bdk_core::bitcoin::{Address, ScriptBuf};
use std::str::FromStr;

const PK_BYTES: &[u8] = &[
    12, 244, 72, 4, 163, 4, 211, 81, 159, 82, 153, 123, 125, 74, 142, 40, 55, 237, 191, 231, 31,
    114, 89, 165, 83, 141, 8, 203, 93, 240, 53, 101,
];

pub fn get_test_spk() -> ScriptBuf {
    let secp = Secp256k1::new();
    let pk = UntweakedPublicKey::from_slice(PK_BYTES).expect("Must be valid PK");
    ScriptBuf::new_p2tr(&secp, pk, None)
}

pub fn test_addresses() -> Vec<Address> {
    [
        "bcrt1qj9f7r8r3p2y0sqf4r3r62qysmkuh0fzep473d2ar7rcz64wqvhssjgf0z4",
        "bcrt1qmm5t0ch7vh2hryx9ctq3mswexcugqe4atkpkl2tetm8merqkthas3w7q30",
        "bcrt1qut9p7ej7l7lhyvekj28xknn8gnugtym4d5qvnp5shrsr4nksmfqsmyn87g",
        "bcrt1qqz0xtn3m235p2k96f5wa2dqukg6shxn9n3txe8arlrhjh5p744hsd957ww",
        "bcrt1q9c0t62a8l6wfytmf2t9lfj35avadk3mm8g4p3l84tp6rl66m48sqrme7wu",
        "bcrt1qkmh8yrk2v47cklt8dytk8f3ammcwa4q7dzattedzfhqzvfwwgyzsg59zrh",
        "bcrt1qvgrsrzy07gjkkfr5luplt0azxtfwmwq5t62gum5jr7zwcvep2acs8hhnp2",
        "bcrt1qw57edarcg50ansq8mk3guyrk78rk0fwvrds5xvqeupteu848zayq549av8",
        "bcrt1qvtve5ekf6e5kzs68knvnt2phfw6a0yjqrlgat392m6zt9jsvyxhqfx67ef",
        "bcrt1qw03ddumfs9z0kcu76ln7jrjfdwam20qtffmkcral3qtza90sp9kqm787uk",
    ]
    .into_iter()
    .map(|s| Address::from_str(s).unwrap().assume_checked())
    .collect()
}
