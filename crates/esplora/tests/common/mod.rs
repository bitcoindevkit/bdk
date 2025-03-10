use bdk_core::bitcoin::key::{Secp256k1, UntweakedPublicKey};
use bdk_core::bitcoin::ScriptBuf;

const PK_BYTES: &[u8] = &[
    12, 244, 72, 4, 163, 4, 211, 81, 159, 82, 153, 123, 125, 74, 142, 40, 55, 237, 191, 231, 31,
    114, 89, 165, 83, 141, 8, 203, 93, 240, 53, 101,
];

#[allow(dead_code)]
pub fn get_test_spk() -> ScriptBuf {
    let secp = Secp256k1::new();
    let pk = UntweakedPublicKey::from_slice(PK_BYTES).expect("Must be valid PK");
    ScriptBuf::new_p2tr(&secp, pk, None)
}
