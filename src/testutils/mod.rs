// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.
#![allow(missing_docs)]

#[cfg(test)]
#[cfg(feature = "test-blockchains")]
pub mod blockchain_tests;

#[cfg(test)]
#[cfg(feature = "test-blockchains")]
pub mod configurable_blockchain_tests;

use bitcoin::{Address, Txid};

#[derive(Clone, Debug)]
pub struct TestIncomingInput {
    pub txid: Txid,
    pub vout: u32,
    pub sequence: Option<u32>,
}

impl TestIncomingInput {
    pub fn new(txid: Txid, vout: u32, sequence: Option<u32>) -> Self {
        Self {
            txid,
            vout,
            sequence,
        }
    }

    #[cfg(feature = "test-blockchains")]
    pub fn into_raw_tx_input(self) -> bitcoincore_rpc::json::CreateRawTransactionInput {
        bitcoincore_rpc::json::CreateRawTransactionInput {
            txid: self.txid,
            vout: self.vout,
            sequence: self.sequence,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestIncomingOutput {
    pub value: u64,
    pub to_address: String,
}

impl TestIncomingOutput {
    pub fn new(value: u64, to_address: Address) -> Self {
        Self {
            value,
            to_address: to_address.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestIncomingTx {
    pub input: Vec<TestIncomingInput>,
    pub output: Vec<TestIncomingOutput>,
    pub min_confirmations: Option<u64>,
    pub locktime: Option<i64>,
    pub replaceable: Option<bool>,
}

impl TestIncomingTx {
    pub fn new(
        input: Vec<TestIncomingInput>,
        output: Vec<TestIncomingOutput>,
        min_confirmations: Option<u64>,
        locktime: Option<i64>,
        replaceable: Option<bool>,
    ) -> Self {
        Self {
            input,
            output,
            min_confirmations,
            locktime,
            replaceable,
        }
    }

    pub fn add_input(&mut self, input: TestIncomingInput) {
        self.input.push(input);
    }

    pub fn add_output(&mut self, output: TestIncomingOutput) {
        self.output.push(output);
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! testutils {
    ( @external $descriptors:expr, $child:expr ) => ({
        use $crate::bitcoin::secp256k1::Secp256k1;
        use $crate::miniscript::descriptor::{Descriptor, DescriptorPublicKey};

        let secp = Secp256k1::new();

        let parsed = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &$descriptors.0).expect("Failed to parse descriptor in `testutils!(@external)`").0;
        parsed.at_derivation_index($child).address(bitcoin::Network::Regtest).expect("No address form")
    });
    ( @internal $descriptors:expr, $child:expr ) => ({
        use $crate::bitcoin::secp256k1::Secp256k1;
        use $crate::miniscript::descriptor::{Descriptor, DescriptorPublicKey};

        let secp = Secp256k1::new();

        let parsed = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &$descriptors.1.expect("Missing internal descriptor")).expect("Failed to parse descriptor in `testutils!(@internal)`").0;
        parsed.at_derivation_index($child).address($crate::bitcoin::Network::Regtest).expect("No address form")
    });
    ( @e $descriptors:expr, $child:expr ) => ({ testutils!(@external $descriptors, $child) });
    ( @i $descriptors:expr, $child:expr ) => ({ testutils!(@internal $descriptors, $child) });
    ( @addr $addr:expr ) => ({ $addr });

    ( @tx ( $( ( $( $addr:tt )* ) => $amount:expr ),+ ) $( ( @inputs $( ($txid:expr, $vout:expr) ),+ ) )? $( ( @locktime $locktime:expr ) )? $( ( @confirmations $confirmations:expr ) )? $( ( @replaceable $replaceable:expr ) )? ) => ({
        let outs = vec![$( $crate::testutils::TestIncomingOutput::new($amount, testutils!( $($addr)* ))),+];
        let _ins: Vec<$crate::testutils::TestIncomingInput> = vec![];
        $(
            let _ins = vec![$( $crate::testutils::TestIncomingInput { txid: $txid, vout: $vout, sequence: None }),+];
        )?

        let locktime = None::<i64>$(.or(Some($locktime)))?;

        let min_confirmations = None::<u64>$(.or(Some($confirmations)))?;
        let replaceable = None::<bool>$(.or(Some($replaceable)))?;

        $crate::testutils::TestIncomingTx::new(_ins, outs, min_confirmations, locktime, replaceable)
    });

    ( @literal $key:expr ) => ({
        let key = $key.to_string();
        (key, None::<String>, None::<String>)
    });
    ( @generate_xprv $( $external_path:expr )? $( ,$internal_path:expr )? ) => ({
        use rand::Rng;

        let mut seed = [0u8; 32];
        rand::thread_rng().fill(&mut seed[..]);

        let key = $crate::bitcoin::util::bip32::ExtendedPrivKey::new_master(
            $crate::bitcoin::Network::Testnet,
            &seed,
        );

        let external_path = None::<String>$(.or(Some($external_path.to_string())))?;
        let internal_path = None::<String>$(.or(Some($internal_path.to_string())))?;

        (key.unwrap().to_string(), external_path, internal_path)
    });
    ( @generate_wif ) => ({
        use rand::Rng;

        let mut key = [0u8; $crate::bitcoin::secp256k1::constants::SECRET_KEY_SIZE];
        rand::thread_rng().fill(&mut key[..]);

        ($crate::bitcoin::PrivateKey {
            compressed: true,
            network: $crate::bitcoin::Network::Testnet,
            key: $crate::bitcoin::secp256k1::SecretKey::from_slice(&key).unwrap(),
        }.to_string(), None::<String>, None::<String>)
    });

    ( @keys ( $( $alias:expr => ( $( $key_type:tt )* ) ),+ ) ) => ({
        let mut map = std::collections::HashMap::new();
        $(
            let alias: &str = $alias;
            map.insert(alias, testutils!( $($key_type)* ));
        )+

        map
    });

    ( @descriptors ( $external_descriptor:expr ) $( ( $internal_descriptor:expr ) )? $( ( @keys $( $keys:tt )* ) )* ) => ({
        use std::str::FromStr;
        use std::collections::HashMap;
        use std::convert::Infallible;

        use $crate::miniscript::descriptor::Descriptor;
        use $crate::miniscript::TranslatePk;

        struct Translator {
            keys: HashMap<&'static str, (String, Option<String>, Option<String>)>,
            is_internal: bool,
        }

        impl $crate::miniscript::Translator<String, String, Infallible> for Translator {
            fn pk(&mut self, pk: &String) -> Result<String, Infallible> {
                match self.keys.get(pk.as_str()) {
                    Some((key, ext_path, int_path)) => {
                        let path = if self.is_internal { int_path } else { ext_path };
                        Ok(format!("{}{}", key, path.clone().unwrap_or_default()))
                    }
                    None => Ok(pk.clone()),
                }
            }
            fn sha256(&mut self, sha256: &String) -> Result<String, Infallible> { Ok(sha256.clone()) }
            fn hash256(&mut self, hash256: &String) -> Result<String, Infallible> { Ok(hash256.clone()) }
            fn ripemd160(&mut self, ripemd160: &String) -> Result<String, Infallible> { Ok(ripemd160.clone()) }
            fn hash160(&mut self, hash160: &String) -> Result<String, Infallible> { Ok(hash160.clone()) }
        }

        #[allow(unused_assignments, unused_mut)]
        let mut keys = HashMap::new();
        $(
            keys = testutils!{ @keys $( $keys )* };
        )*

        let mut translator = Translator { keys, is_internal: false };

        let external: Descriptor<String> = FromStr::from_str($external_descriptor).unwrap();
        let external = external.translate_pk(&mut translator).expect("Infallible conversion");
        let external = external.to_string();

        translator.is_internal = true;

        let internal = None::<String>$(.or({
            let internal: Descriptor<String> = FromStr::from_str($internal_descriptor).unwrap();
            let internal = internal.translate_pk(&mut translator).expect("Infallible conversion");
            Some(internal.to_string())
        }))?;

        (external, internal)
    })
}
