// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

extern crate bdk;
extern crate serde_json;

use std::sync::Arc;

use bdk::bitcoin::util::bip32::ChildNumber;
use bdk::bitcoin::*;
use bdk::descriptor::*;

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

    let derived_desc = extended_desc.derive(ChildNumber::from_normal_idx(42).unwrap());
    println!("{:?}", derived_desc);

    let addr = derived_desc.address(Network::Testnet).unwrap();
    println!("{}", addr);

    let script = derived_desc.witness_script();
    println!("{:?}", script);
}
