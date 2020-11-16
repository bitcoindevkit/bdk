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

//! Descriptors DSL

#[doc(hidden)]
#[macro_export]
macro_rules! impl_top_level_sh {
    ( $descriptor_variant:ident, $( $minisc:tt )* ) => {
        $crate::fragment!($( $minisc )*)
            .map(|(minisc, keymap, networks)|($crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::$descriptor_variant(minisc), keymap, networks))
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_top_level_pk {
    ( $descriptor_variant:ident, $ctx:ty, $key:expr ) => {{
        #[allow(unused_imports)]
        use $crate::keys::{DescriptorKey, ToDescriptorKey};
        let secp = $crate::bitcoin::secp256k1::Secp256k1::new();

        $key.to_descriptor_key()
            .and_then(|key: DescriptorKey<$ctx>| key.extract(&secp))
            .map(|(pk, key_map, valid_networks)| {
                (
                    $crate::miniscript::Descriptor::<
                        $crate::miniscript::descriptor::DescriptorPublicKey,
                    >::$descriptor_variant(pk),
                    key_map,
                    valid_networks,
                )
            })
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_modifier {
    ( $terminal_variant:ident, $( $inner:tt )* ) => {
        $crate::fragment!($( $inner )*)
            .map_err(|e| -> $crate::Error { e.into() })
            .and_then(|(minisc, keymap, networks)| Ok(($crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::$terminal_variant(std::sync::Arc::new(minisc)))?, keymap, networks)))
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_leaf_opcode {
    ( $terminal_variant:ident ) => {
        $crate::miniscript::Miniscript::from_ast(
            $crate::miniscript::miniscript::decode::Terminal::$terminal_variant,
        )
        .map_err($crate::Error::Miniscript)
        .map(|minisc| {
            (
                minisc,
                $crate::miniscript::descriptor::KeyMap::default(),
                $crate::keys::any_network(),
            )
        })
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_leaf_opcode_value {
    ( $terminal_variant:ident, $value:expr ) => {
        $crate::miniscript::Miniscript::from_ast(
            $crate::miniscript::miniscript::decode::Terminal::$terminal_variant($value),
        )
        .map_err($crate::Error::Miniscript)
        .map(|minisc| {
            (
                minisc,
                $crate::miniscript::descriptor::KeyMap::default(),
                $crate::keys::any_network(),
            )
        })
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_leaf_opcode_value_two {
    ( $terminal_variant:ident, $one:expr, $two:expr ) => {
        $crate::miniscript::Miniscript::from_ast(
            $crate::miniscript::miniscript::decode::Terminal::$terminal_variant($one, $two),
        )
        .map_err($crate::Error::Miniscript)
        .map(|minisc| {
            (
                minisc,
                $crate::miniscript::descriptor::KeyMap::default(),
                $crate::keys::any_network(),
            )
        })
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_node_opcode_two {
    ( $terminal_variant:ident, ( $( $a:tt )* ), ( $( $b:tt )* ) ) => {
        $crate::fragment!($( $a )*)
            .and_then(|a| Ok((a, $crate::fragment!($( $b )*)?)))
            .and_then(|((a_minisc, mut a_keymap, a_networks), (b_minisc, b_keymap, b_networks))| {
                // join key_maps
                a_keymap.extend(b_keymap.into_iter());

                Ok(($crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::$terminal_variant(
                    std::sync::Arc::new(a_minisc),
                    std::sync::Arc::new(b_minisc),
                ))?, a_keymap, $crate::keys::merge_networks(&a_networks, &b_networks)))
            })
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! impl_node_opcode_three {
    ( $terminal_variant:ident, ( $( $a:tt )* ), ( $( $b:tt )* ), ( $( $c:tt )* ) ) => {
        $crate::fragment!($( $a )*)
            .and_then(|a| Ok((a, $crate::fragment!($( $b )*)?, $crate::fragment!($( $c )*)?)))
            .and_then(|((a_minisc, mut a_keymap, a_networks), (b_minisc, b_keymap, b_networks), (c_minisc, c_keymap, c_networks))| {
                // join key_maps
                a_keymap.extend(b_keymap.into_iter());
                a_keymap.extend(c_keymap.into_iter());

                let networks = $crate::keys::merge_networks(&a_networks, &b_networks);
                let networks = $crate::keys::merge_networks(&networks, &c_networks);

                Ok(($crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::$terminal_variant(
                    std::sync::Arc::new(a_minisc),
                    std::sync::Arc::new(b_minisc),
                    std::sync::Arc::new(c_minisc),
                ))?, a_keymap, networks))
            })
    };
}

/// Macro to write full descriptors with code
///
/// This macro expands to an object of type `Result<(Descriptor<DescriptorPublicKey>, KeyMap, ValidNetworks), Error>`.
///
/// ## Example
///
/// Signature plus timelock, equivalent to: `sh(wsh(and_v(v:pk(...), older(...))))`
///
/// ```
/// # use std::str::FromStr;
/// let my_key = bitcoin::PublicKey::from_str("02e96fe52ef0e22d2f131dd425ce1893073a3c6ad20e8cac36726393dfb4856a4c")?;
/// let my_timelock = 50;
/// let (my_descriptor, my_keys_map, networks) = bdk::descriptor!(sh ( wsh ( and_v (+v pk my_key), ( older my_timelock ))))?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// -------
///
/// 2-of-3 that becomes a 1-of-3 after a timelock has expired. Both `descriptor_a` and `descriptor_b` are equivalent: the first
/// syntax is more suitable for a fixed number of items known at compile time, while the other accepts a
/// [`Vec`] of items, which makes it more suitable for writing dynamic descriptors.
///
/// They both produce the descriptor: `wsh(thresh(2,pk(...),s:pk(...),sdv:older(...)))`
///
/// ```
/// # use std::str::FromStr;
/// let my_key_1 = bitcoin::PublicKey::from_str("02e96fe52ef0e22d2f131dd425ce1893073a3c6ad20e8cac36726393dfb4856a4c")?;
/// let my_key_2 = bitcoin::PrivateKey::from_wif("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy")?;
/// let my_timelock = 50;
///
/// let (descriptor_a, key_map_a, networks) = bdk::descriptor! {
///     wsh (
///         thresh 2, (pk my_key_1), (+s pk my_key_2), (+s+d+v older my_timelock)
///     )
/// }?;
///
/// let b_items = vec![
///     bdk::fragment!(pk my_key_1)?,
///     bdk::fragment!(+s pk my_key_2)?,
///     bdk::fragment!(+s+d+v older my_timelock)?,
/// ];
/// let (descriptor_b, mut key_map_b, networks) = bdk::descriptor!( wsh ( thresh_vec 2, b_items ) )?;
///
/// assert_eq!(descriptor_a, descriptor_b);
/// assert_eq!(key_map_a.len(), key_map_b.len());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ------
///
/// Simple 2-of-2 multi-signature, equivalent to: `wsh(multi(2, ...))`
///
/// ```
/// # use std::str::FromStr;
/// let my_key_1 = bitcoin::PublicKey::from_str("02e96fe52ef0e22d2f131dd425ce1893073a3c6ad20e8cac36726393dfb4856a4c")?;
/// let my_key_2 = bitcoin::PrivateKey::from_wif("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy")?;
///
/// let (descriptor, key_map, networks) = bdk::descriptor! {
///     wsh (
///         multi 2, my_key_1, my_key_2
///     )
/// }?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ------
///
/// Native-Segwit single-sig, equivalent to: `wpkh(...)`
///
/// ```
/// let my_key = bitcoin::PrivateKey::from_wif("cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy")?;
///
/// let (descriptor, key_map, networks) = bdk::descriptor!(wpkh ( my_key ) )?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[macro_export]
macro_rules! descriptor {
    ( bare ( $( $minisc:tt )* ) ) => ({
        $crate::impl_top_level_sh!(Bare, $( $minisc )*)
    });
    ( sh ( wsh ( $( $minisc:tt )* ) ) ) => ({
        $crate::descriptor!(shwsh ($( $minisc )*))
    });
    ( shwsh ( $( $minisc:tt )* ) ) => ({
        $crate::impl_top_level_sh!(ShWsh, $( $minisc )*)
    });
    ( pk $key:expr ) => ({
        $crate::impl_top_level_pk!(Pk, $crate::miniscript::Legacy, $key)
    });
    ( pkh $key:expr ) => ({
        $crate::impl_top_level_pk!(Pkh,$crate::miniscript::Legacy, $key)
    });
    ( wpkh $key:expr ) => ({
        $crate::impl_top_level_pk!(Wpkh, $crate::miniscript::Segwitv0, $key)
    });
    ( sh ( wpkh ( $key:expr ) ) ) => ({
        $crate::descriptor!(shwpkh ( $key ))
    });
    ( shwpkh ( $key:expr ) ) => ({
        $crate::impl_top_level_pk!(ShWpkh, $crate::miniscript::Segwitv0, $key)
    });
    ( sh ( $( $minisc:tt )* ) ) => ({
        $crate::impl_top_level_sh!(Sh, $( $minisc )*)
    });
    ( wsh ( $( $minisc:tt )* ) ) => ({
        $crate::impl_top_level_sh!(Wsh, $( $minisc )*)
    });
}

/// Macro to write descriptor fragments with code
///
/// This macro will be expanded to an object of type `Result<(Miniscript<DescriptorPublicKey, _>, KeyMap, ValidNetworks), Error>`. It allows writing
/// fragments of larger descriptors that can be pieced together using `fragment!(thresh_vec ...)`.
#[macro_export]
macro_rules! fragment {
    // Modifiers
    ( +a $( $inner:tt )* ) => ({
        $crate::impl_modifier!(Alt, $( $inner )*)
    });
    ( +s $( $inner:tt )* ) => ({
        $crate::impl_modifier!(Swap, $( $inner )*)
    });
    ( +c $( $inner:tt )* ) => ({
        $crate::impl_modifier!(Check, $( $inner )*)
    });
    ( +d $( $inner:tt )* ) => ({
        $crate::impl_modifier!(DupIf, $( $inner )*)
    });
    ( +v $( $inner:tt )* ) => ({
        $crate::impl_modifier!(Verify, $( $inner )*)
    });
    ( +j $( $inner:tt )* ) => ({
        $crate::impl_modifier!(NonZero, $( $inner )*)
    });
    ( +n $( $inner:tt )* ) => ({
        $crate::impl_modifier!(ZeroNotEqual, $( $inner )*)
    });
    ( +t $( $inner:tt )* ) => ({
        $crate::fragment!(and_v ( $( $inner )* ), ( true ) )
    });
    ( +l $( $inner:tt )* ) => ({
        $crate::fragment!(or_i ( false ), ( $( $inner )* ) )
    });
    ( +u $( $inner:tt )* ) => ({
        $crate::fragment!(or_i ( $( $inner )* ), ( false ) )
    });

    // Miniscript
    ( true ) => ({
        $crate::impl_leaf_opcode!(True)
    });
    ( false ) => ({
        $crate::impl_leaf_opcode!(False)
    });
    ( pk_k $key:expr ) => ({
        let secp = $crate::bitcoin::secp256k1::Secp256k1::new();
        $crate::keys::make_pk($key, &secp)
    });
    ( pk $key:expr ) => ({
        $crate::fragment!(+c pk_k $key)
    });
    ( pk_h $key_hash:expr ) => ({
        $crate::impl_leaf_opcode_value!(PkH, $key_hash)
    });
    ( after $value:expr ) => ({
        $crate::impl_leaf_opcode_value!(After, $value)
    });
    ( older $value:expr ) => ({
        $crate::impl_leaf_opcode_value!(Older, $value)
    });
    ( sha256 $hash:expr ) => ({
        $crate::impl_leaf_opcode_value!(Sha256, $hash)
    });
    ( hash256 $hash:expr ) => ({
        $crate::impl_leaf_opcode_value!(Hash256, $hash)
    });
    ( ripemd160 $hash:expr ) => ({
        $crate::impl_leaf_opcode_value!(Ripemd160, $hash)
    });
    ( hash160 $hash:expr ) => ({
        $crate::impl_leaf_opcode_value!(Hash160, $hash)
    });
    ( and_v ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::impl_node_opcode_two!(AndV, ( $( $a )* ), ( $( $b )* ))
    });
    ( and_b ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::impl_node_opcode_two!(AndB, ( $( $a )* ), ( $( $b )* ))
    });
    ( and_or ( $( $a:tt )* ), ( $( $b:tt )* ), ( $( $c:tt )* ) ) => ({
        $crate::impl_node_opcode_three!(AndOr, ( $( $a )* ), ( $( $b )* ), ( $( $c )* ))
    });
    ( or_b ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::impl_node_opcode_two!(OrB, ( $( $a )* ), ( $( $b )* ))
    });
    ( or_d ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::impl_node_opcode_two!(OrD, ( $( $a )* ), ( $( $b )* ))
    });
    ( or_c ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::impl_node_opcode_two!(OrC, ( $( $a )* ), ( $( $b )* ))
    });
    ( or_i ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::impl_node_opcode_two!(OrI, ( $( $a )* ), ( $( $b )* ))
    });
    ( thresh_vec $thresh:expr, $items:expr ) => ({
        use $crate::miniscript::descriptor::KeyMap;

        let (items, key_maps_networks): (Vec<_>, Vec<_>) = $items.into_iter().map(|(a, b, c)| (a, (b, c))).unzip();
        let items = items.into_iter().map(std::sync::Arc::new).collect();

        let (key_maps, valid_networks) = key_maps_networks.into_iter().fold((KeyMap::default(), $crate::keys::any_network()), |(mut keys_acc, net_acc), (key, net)| {
            keys_acc.extend(key.into_iter());
            let net_acc = $crate::keys::merge_networks(&net_acc, &net);

            (keys_acc, net_acc)
        });

        $crate::impl_leaf_opcode_value_two!(Thresh, $thresh, items)
            .map(|(minisc, _, _)| (minisc, key_maps, valid_networks))
    });
    ( thresh $thresh:expr $(, ( $( $item:tt )* ) )+ ) => ({
        let mut items = vec![];
        $(
            items.push($crate::fragment!($( $item )*));
        )*

        items.into_iter().collect::<Result<Vec<_>, _>>()
            .and_then(|items| $crate::fragment!(thresh_vec $thresh, items))
    });
    ( multi_vec $thresh:expr, $keys:expr ) => ({
        $crate::keys::make_multi($thresh, $keys)
    });
    ( multi $thresh:expr $(, $key:expr )+ ) => ({
        use $crate::keys::ToDescriptorKey;
        let secp = $crate::bitcoin::secp256k1::Secp256k1::new();

        let mut keys = vec![];
        $(
            keys.push($key.to_descriptor_key());
        )*

        keys.into_iter().collect::<Result<Vec<_>, _>>()
            .and_then(|keys| $crate::keys::make_multi($thresh, keys, &secp))
    });

}

#[cfg(test)]
mod test {
    use bitcoin::hashes::hex::ToHex;
    use bitcoin::secp256k1::Secp256k1;
    use miniscript::descriptor::{DescriptorPublicKey, DescriptorPublicKeyCtx, KeyMap};
    use miniscript::{Descriptor, Legacy, Segwitv0};

    use std::str::FromStr;

    use crate::descriptor::DescriptorMeta;
    use crate::keys::{DescriptorKey, KeyError, ToDescriptorKey, ValidNetworks};
    use bitcoin::network::constants::Network::{Bitcoin, Regtest, Testnet};
    use bitcoin::util::bip32;
    use bitcoin::util::bip32::ChildNumber;

    // test the descriptor!() macro

    // verify descriptor generates expected script(s) (if bare or pk) or address(es)
    fn check(
        desc: Result<(Descriptor<DescriptorPublicKey>, KeyMap, ValidNetworks), KeyError>,
        is_witness: bool,
        is_fixed: bool,
        expected: &[&str],
    ) {
        let secp = Secp256k1::new();
        let deriv_ctx = DescriptorPublicKeyCtx::new(&secp, ChildNumber::Normal { index: 0 });

        let (desc, _key_map, _networks) = desc.unwrap();
        assert_eq!(desc.is_witness(), is_witness);
        assert_eq!(desc.is_fixed(), is_fixed);
        for i in 0..expected.len() {
            let index = i as u32;
            let child_desc = if desc.is_fixed() {
                desc.clone()
            } else {
                desc.derive(ChildNumber::from_normal_idx(index).unwrap())
            };
            let address = child_desc.address(Regtest, deriv_ctx);
            if address.is_some() {
                assert_eq!(address.unwrap().to_string(), *expected.get(i).unwrap());
            } else {
                let script = child_desc.script_pubkey(deriv_ctx);
                assert_eq!(script.to_hex().as_str(), *expected.get(i).unwrap());
            }
        }
    }

    // - at least one of each "type" of operator; ie. one modifier, one leaf_opcode, one leaf_opcode_value, etc.
    // - mixing up key types that implement ToDescriptorKey in multi() or thresh()

    // expected script for pk and bare manually created
    // expected addresses created with `bitcoin-cli getdescriptorinfo` (for hash) and `bitcoin-cli deriveaddresses`

    #[test]
    fn test_fixed_legacy_descriptors() {
        let pubkey1 = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        let pubkey2 = bitcoin::PublicKey::from_str(
            "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af",
        )
        .unwrap();

        check(
            descriptor!(bare(multi 1,pubkey1,pubkey2)),
            false,
            true,
            &["512103a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd21032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af52ae"],
        );
        check(
            descriptor!(pk(pubkey1)),
            false,
            true,
            &["2103a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bdac"],
        );
        check(
            descriptor!(pkh(pubkey1)),
            false,
            true,
            &["muZpTpBYhxmRFuCjLc7C6BBDF32C8XVJUi"],
        );
        check(
            descriptor!(sh(multi 1,pubkey1,pubkey2)),
            false,
            true,
            &["2MymURoV1bzuMnWMGiXzyomDkeuxXY7Suey"],
        );
    }

    #[test]
    fn test_fixed_segwitv0_descriptors() {
        let pubkey1 = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        let pubkey2 = bitcoin::PublicKey::from_str(
            "032e58afe51f9ed8ad3cc7897f634d881fdbe49a81564629ded8156bebd2ffd1af",
        )
        .unwrap();

        check(
            descriptor!(wpkh(pubkey1)),
            true,
            true,
            &["bcrt1qngw83fg8dz0k749cg7k3emc7v98wy0c7azaa6h"],
        );
        check(
            descriptor!(sh(wpkh(pubkey1))),
            true,
            true,
            &["2N5LiC3CqzxDamRTPG1kiNv1FpNJQ7x28sb"],
        );
        check(
            descriptor!(wsh(multi 1,pubkey1,pubkey2)),
            true,
            true,
            &["bcrt1qgw8jvv2hsrvjfa6q66rk6har7d32lrqm5unnf5cl63q9phxfvgps5fyfqe"],
        );
        check(
            descriptor!(sh(wsh(multi 1,pubkey1,pubkey2))),
            true,
            true,
            &["2NCidRJysy7apkmE6JF5mLLaJFkrN3Ub9iy"],
        );
    }

    #[test]
    fn test_bip32_legacy_descriptors() {
        let xprv = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();

        let path = bip32::DerivationPath::from_str("m/0").unwrap();
        let desc_key = (xprv, path.clone()).to_descriptor_key().unwrap();
        check(
            descriptor!(pk(desc_key)),
            false,
            false,
            &[
                "2102363ad03c10024e1b597a5b01b9982807fb638e00b06f3b2d4a89707de3b93c37ac",
                "2102063a21fd780df370ed2fc8c4b86aa5ea642630609c203009df631feb7b480dd2ac",
                "2102ba2685ad1fa5891cb100f1656b2ce3801822ccb9bac0336734a6f8c1b93ebbc0ac",
            ],
        );

        let desc_key = (xprv, path.clone()).to_descriptor_key().unwrap();
        check(
            descriptor!(pkh(desc_key)),
            false,
            false,
            &[
                "muvBdsVpJxpFuTHMKA47htJPdCvdt4F9DP",
                "mxQSHK7DL2t1DN3xFxov1janCoXSSkrSPj",
                "mfz43r15GiWo4nizmyzMNubsnkDpByFFAn",
            ],
        );

        let path2 = bip32::DerivationPath::from_str("m/2147483647'/0").unwrap();
        let desc_key1 = (xprv, path).to_descriptor_key().unwrap();
        let desc_key2 = (xprv, path2).to_descriptor_key().unwrap();

        check(
            descriptor!(sh(multi 1,desc_key1,desc_key2)),
            false,
            false,
            &[
                "2MtMDXsfwefZkEEhVViEPidvcKRUtJamJJ8",
                "2MwAUZ1NYyWjhVvGTethFL6n7nZhS8WE6At",
                "2MuT6Bj66HLwZd7s4SoD8XbK4GwriKEA6Gr",
            ],
        );
    }

    #[test]
    fn test_bip32_segwitv0_descriptors() {
        let xprv = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();

        let path = bip32::DerivationPath::from_str("m/0").unwrap();
        let desc_key = (xprv, path.clone()).to_descriptor_key().unwrap();
        check(
            descriptor!(wpkh(desc_key)),
            true,
            false,
            &[
                "bcrt1qnhm8w9fhc8cxzgqsmqdf9fyjccyvc0gltnymu0",
                "bcrt1qhylfd55rn75w9fj06zspctad5w4hz33rf0ttad",
                "bcrt1qq5sq3a6k9av9d8cne0k9wcldy4nqey5yt6889r",
            ],
        );

        let desc_key = (xprv, path.clone()).to_descriptor_key().unwrap();
        check(
            descriptor!(sh(wpkh(desc_key))),
            true,
            false,
            &[
                "2MxvjQCaLqZ5QxZ7XotZDQ63hZw3NPss763",
                "2NDUoevN4QMzhvHDMGhKuiT2fN9HXbFRMwn",
                "2NF4BEAY2jF1Fu8vqfN3NVKoFtom77pUxrx",
            ],
        );

        let path2 = bip32::DerivationPath::from_str("m/2147483647'/0").unwrap();
        let desc_key1 = (xprv, path.clone()).to_descriptor_key().unwrap();
        let desc_key2 = (xprv, path2.clone()).to_descriptor_key().unwrap();
        check(
            descriptor!(wsh(multi 1,desc_key1,desc_key2)),
            true,
            false,
            &[
                "bcrt1qfxv8mxmlv5sz8q2mnuyaqdfe9jr4vvmx0csjhn092p6f4qfygfkq2hng49",
                "bcrt1qerj85g243e6jlcdxpmn9spk0gefcwvu7nw7ee059d5ydzpdhkm2qwfkf5k",
                "bcrt1qxkl2qss3k58q9ktc8e89pwr4gnptfpw4hju4xstxcjc0hkcae3jstluty7",
            ],
        );

        let desc_key1 = (xprv, path).to_descriptor_key().unwrap();
        let desc_key2 = (xprv, path2).to_descriptor_key().unwrap();
        check(
            descriptor!(sh(wsh(multi 1,desc_key1,desc_key2))),
            true,
            false,
            &[
                "2NFCtXvx9q4ci2kvKub17iSTgvRXGctCGhz",
                "2NB2PrFPv5NxWCpygas8tPrGJG2ZFgeuwJw",
                "2N79ZAGo5cMi5Jt7Wo9L5YmF5GkEw7sjWdC",
            ],
        );
    }

    // - verify the valid_networks returned is correctly computed based on the keys present in the descriptor
    #[test]
    fn test_valid_networks() {
        let xprv = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        let path = bip32::DerivationPath::from_str("m/0").unwrap();
        let desc_key = (xprv, path.clone()).to_descriptor_key().unwrap();

        let (_desc, _key_map, valid_networks) = descriptor!(pkh(desc_key)).unwrap();
        assert_eq!(valid_networks, [Testnet, Regtest].iter().cloned().collect());

        let xprv = bip32::ExtendedPrivKey::from_str("xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi").unwrap();
        let path = bip32::DerivationPath::from_str("m/10/20/30/40").unwrap();
        let desc_key = (xprv, path.clone()).to_descriptor_key().unwrap();

        let (_desc, _key_map, valid_networks) = descriptor!(wpkh(desc_key)).unwrap();
        assert_eq!(valid_networks, [Bitcoin].iter().cloned().collect());
    }

    // - verify the key_maps are correctly merged together
    #[test]
    fn test_key_maps_merged() {
        let secp = Secp256k1::new();

        let xprv1 = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        let path1 = bip32::DerivationPath::from_str("m/0").unwrap();
        let desc_key1 = (xprv1, path1.clone()).to_descriptor_key().unwrap();

        let xprv2 = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPegBHHnq7YEgM815dG24M2Jk5RVqipgDxF1HJ1tsnT815X5Fd5FRfMVUs8NZs9XCb6y9an8hRPThnhfwfXJ36intaekySHGF").unwrap();
        let path2 = bip32::DerivationPath::from_str("m/2147483647'/0").unwrap();
        let desc_key2 = (xprv2, path2.clone()).to_descriptor_key().unwrap();

        let xprv3 = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPdZXrcHNLf5JAJWFAoJ2TrstMRdSKtEggz6PddbuSkvHKM9oKJyFgZV1B7rw8oChspxyYbtmEXYyg1AjfWbL3ho3XHDpHRZf").unwrap();
        let path3 = bip32::DerivationPath::from_str("m/10/20/30/40").unwrap();
        let desc_key3 = (xprv3, path3.clone()).to_descriptor_key().unwrap();

        let (_desc, key_map, _valid_networks) =
            descriptor!(sh(wsh(multi 2,desc_key1,desc_key2,desc_key3))).unwrap();
        assert_eq!(key_map.len(), 3);

        let desc_key1: DescriptorKey<Segwitv0> =
            (xprv1, path1.clone()).to_descriptor_key().unwrap();
        let desc_key2: DescriptorKey<Segwitv0> =
            (xprv2, path2.clone()).to_descriptor_key().unwrap();
        let desc_key3: DescriptorKey<Segwitv0> =
            (xprv3, path3.clone()).to_descriptor_key().unwrap();

        let (key1, _key_map, _valid_networks) = desc_key1.extract(&secp).unwrap();
        let (key2, _key_map, _valid_networks) = desc_key2.extract(&secp).unwrap();
        let (key3, _key_map, _valid_networks) = desc_key3.extract(&secp).unwrap();
        assert_eq!(key_map.get(&key1).unwrap().to_string(), "tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy/0/*");
        assert_eq!(key_map.get(&key2).unwrap().to_string(), "tprv8ZgxMBicQKsPegBHHnq7YEgM815dG24M2Jk5RVqipgDxF1HJ1tsnT815X5Fd5FRfMVUs8NZs9XCb6y9an8hRPThnhfwfXJ36intaekySHGF/2147483647'/0/*");
        assert_eq!(key_map.get(&key3).unwrap().to_string(), "tprv8ZgxMBicQKsPdZXrcHNLf5JAJWFAoJ2TrstMRdSKtEggz6PddbuSkvHKM9oKJyFgZV1B7rw8oChspxyYbtmEXYyg1AjfWbL3ho3XHDpHRZf/10/20/30/40/*");
    }

    // - verify the ScriptContext is correctly validated (i.e. passing a type that only impl ToDescriptorKey<Segwitv0> to a pkh() descriptor should throw a compilation error
    #[test]
    fn test_script_context_validation() {
        // this compiles
        let xprv = bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        let path = bip32::DerivationPath::from_str("m/0").unwrap();
        let desc_key: DescriptorKey<Legacy> = (xprv, path.clone()).to_descriptor_key().unwrap();

        let (desc, _key_map, _valid_networks) = descriptor!(pkh(desc_key)).unwrap();
        assert_eq!(desc.to_string(), "pkh(tpubD6NzVbkrYhZ4WR7a4vY1VT3khMJMeAxVsfq9TBJyJWrNk247zCJtV7AWf6UJP7rAVsn8NNKdJi3gFyKPTmWZS9iukb91xbn2HbFSMQm2igY/0/*)");

        // as expected this does not compile due to invalid context
        //let desc_key:DescriptorKey<Segwitv0> = (xprv, path.clone()).to_descriptor_key().unwrap();
        //let (desc, _key_map, _valid_networks) = descriptor!(pkh(desc_key)).unwrap();
    }
}
