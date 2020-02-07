extern crate magical_bitcoin_wallet;
extern crate serde_json;

use std::str::FromStr;

use magical_bitcoin_wallet::bitcoin::*;
use magical_bitcoin_wallet::descriptor::*;

fn main() {
    let desc = "wsh(or_d(\
                    thresh_m(\
                      2,[d34db33f/44'/0'/0']xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/*,tprv8ZgxMBicQKsPduL5QnGihpprdHyypMGi4DhimjtzYemu7se5YQNcZfAPLqXRuGHb5ZX2eTQj62oNqMnyxJ7B7wz54Uzswqw8fFqMVdcmVF7/1/*\
                    ),\
                    and_v(vc:pk_h(cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy),older(1000))\
                   ))";

    let extended_desc = ExtendedDescriptor::from_str(desc).unwrap();
    println!("{:?}", extended_desc);

    let derived_desc = extended_desc.derive(42).unwrap();
    println!("{:?}", derived_desc);

    if let Descriptor::Wsh(x) = &derived_desc {
        println!("{}", serde_json::to_string(&x.extract_policy()).unwrap());
    }

    let addr = derived_desc.address(Network::Testnet).unwrap();
    println!("{}", addr);

    let script = derived_desc.witness_script();
    println!("{:?}", script);
}
