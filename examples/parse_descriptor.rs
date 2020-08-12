extern crate magical_bitcoin_wallet;
extern crate serde_json;

use std::sync::Arc;

use magical_bitcoin_wallet::bitcoin::util::bip32::ChildNumber;
use magical_bitcoin_wallet::bitcoin::*;
use magical_bitcoin_wallet::descriptor::*;

fn main() {
    let desc = "wsh(or_d(\
                    multi(\
                      2,[d34db33f/44'/0'/0']xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/*,tprv8ZgxMBicQKsPduL5QnGihpprdHyypMGi4DhimjtzYemu7se5YQNcZfAPLqXRuGHb5ZX2eTQj62oNqMnyxJ7B7wz54Uzswqw8fFqMVdcmVF7/1/*\
                    ),\
                    and_v(vc:pk_h(cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy),older(1000))\
                   ))";

    let (extended_desc, key_map) = ExtendedDescriptor::parse_secret(desc).unwrap();
    println!("{:?}", extended_desc);

    let signers = Arc::new(key_map.into());
    let policy = extended_desc.extract_policy(signers).unwrap();
    println!("policy: {}", serde_json::to_string(&policy).unwrap());

    let derived_desc = extended_desc.derive(&[ChildNumber::from_normal_idx(42).unwrap()]);
    println!("{:?}", derived_desc);

    let addr = derived_desc.address(Network::Testnet).unwrap();
    println!("{}", addr);

    let script = derived_desc.witness_script();
    println!("{:?}", script);
}
