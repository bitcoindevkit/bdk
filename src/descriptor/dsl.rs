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

//! Descriptor DSL

/// Domain specific language to write descriptors with code
///
/// This macro must be called in a function that returns a `Result<_, E: From<miniscript::Error>>`.
///
/// The `pk()` descriptor type (single sig with a bare public key) is not available, since it could
/// cause conflicts with the equally-named `pk()` descriptor opcode. It has been replaced by `pkr()`,
/// which stands for "public key root".
///
/// ## Example
///
/// Signature plus timelock, equivalent to: `sh(wsh(and_v(v:pk(...), older(...))))`
///
/// ```
/// # use std::str::FromStr;
/// # use miniscript::descriptor::DescriptorPublicKey;
/// let my_key = DescriptorPublicKey::from_str("02e96fe52ef0e22d2f131dd425ce1893073a3c6ad20e8cac36726393dfb4856a4c").unwrap();
/// let my_timelock = 50;
/// let my_descriptor = bdk::desc!(sh ( wsh ( and_v (+v pk my_key), ( older my_timelock ))));
/// # Ok::<(), bdk::Error>(())
/// ```
///
/// 2-of-3 that becomes a 1-of-3 after a timelock has expired. Both `descriptor_a` and `descriptor_b` are equivalent: the first
/// syntax is more suitable for a fixed number of items known at compile time, while the other accepts a
/// [`Vec`] of items, which makes it more suitable for writing dynamic descriptors.
///
/// They both produce the descriptor: `wsh(thresh(2,pk(...),s:pk(...),sdv:older(...)))`
///
/// ```
/// # use std::str::FromStr;
/// # use miniscript::descriptor::DescriptorPublicKey;
/// let my_key = DescriptorPublicKey::from_str("02e96fe52ef0e22d2f131dd425ce1893073a3c6ad20e8cac36726393dfb4856a4c").unwrap();
/// let my_timelock = 50;
///
/// let descriptor_a = bdk::desc! {
///     wsh (
///         thresh 2, (pk my_key.clone()), (+s pk my_key.clone()), (+s+d+v older my_timelock)
///     )
/// };
///
/// let b_items = vec![
///     bdk::desc!(pk my_key.clone()),
///     bdk::desc!(+s pk my_key.clone()),
///     bdk::desc!(+s+d+v older my_timelock),
/// ];
/// let descriptor_b = bdk::desc!( wsh ( thresh_vec 2, b_items ) );
///
/// assert_eq!(descriptor_a, descriptor_b);
/// # Ok::<(), bdk::Error>(())
/// ```
#[macro_export]
macro_rules! desc {
    // Descriptor
    ( bare ( $( $minisc:tt )* ) ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::Bare($crate::desc!($( $minisc )*))
    });
    ( sh ( wsh ( $( $minisc:tt )* ) ) ) => ({
        $crate::desc!(shwsh ($( $minisc )*))
    });
    ( shwsh ( $( $minisc:tt )* ) ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::ShWsh($crate::desc!($( $minisc )*))
    });
    ( pkr $key:expr ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::Pk($key)
    });
    ( pkh $key:expr ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::Pkh($key)
    });
    ( wpkh $key:expr ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::Wpkh($key)
    });
    ( sh ( wpkh ( $key:expr ) ) ) => ({
        $crate::desc!(shwpkh ($( $minisc )*))
    });
    ( shwpkh ( $key:expr ) ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::ShWpkh($key)
    });
    ( sh ( $( $minisc:tt )* ) ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::Sh($crate::desc!($( $minisc )*))
    });
    ( wsh ( $( $minisc:tt )* ) ) => ({
        $crate::miniscript::Descriptor::<$crate::miniscript::descriptor::DescriptorPublicKey>::Wsh($crate::desc!($( $minisc )*))
    });

    // Modifiers
    ( +a $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Alt(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +s $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Swap(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +c $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Check(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +d $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::DupIf(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +v $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Verify(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +j $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::NonZero(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +n $( $inner:tt )* ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::ZeroNotEqual(std::sync::Arc::new($crate::desc!($( $inner )*))))?
    });
    ( +t $( $inner:tt )* ) => ({
        $crate::desc!(and_v ( $( $inner )* ), ( true ) )
    });
    ( +l $( $inner:tt )* ) => ({
        $crate::desc!(or_i ( false ), ( $( $inner )* ) )
    });
    ( +u $( $inner:tt )* ) => ({
        $crate::desc!(or_i ( $( $inner )* ), ( false ) )
    });

    // Miniscript
    ( true ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::True)?
    });
    ( false ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::False)?
    });
    ( pk_k $key:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::PkK($key))?
    });
    ( pk $key:expr ) => ({
        $crate::desc!(+c pk_k $key)
    });
    ( pk_h $key_hash:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::PkH($key_hash))?
    });
    ( after $value:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::After($value))?
    });
    ( older $value:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Older($value))?
    });
    ( sha256 $hash:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Sha256($hash))?
    });
    ( hash256 $hash:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Hash256($hash))?
    });
    ( ripemd160 $hash:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Ripemd160($hash))?
    });
    ( hash160 $hash:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Hash160($hash))?
    });
    ( and_v ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::AndV(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*)),
        ))?
    });
    ( and_b ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::AndB(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*))),
        )?
    });
    ( and_or ( $( $a:tt )* ), ( $( $b:tt )* ), ( $( $c:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::AndOr(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*)),
            std::sync::Arc::new($crate::desc!($( $c )*)),
        ))?
    });
    ( or_b ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::OrB(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*)),
        ))?
    });
    ( or_d ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::OrD(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*)),
        ))?
    });
    ( or_c ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::OrC(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*)),
        ))?
    });
    ( or_i ( $( $a:tt )* ), ( $( $b:tt )* ) ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::OrI(
            std::sync::Arc::new($crate::desc!($( $a )*)),
            std::sync::Arc::new($crate::desc!($( $b )*)),
        ))?
    });
    ( thresh_vec $thresh:expr, $items:expr ) => ({
        let items = $items.into_iter().map(std::sync::Arc::new).collect();
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Thresh($thresh, items))?
    });
    ( thresh $thresh:expr $(, ( $( $item:tt )* ) )+ ) => ({
        let mut items = vec![];
        $(
            items.push(std::sync::Arc::new($crate::desc!($( $item )*)));
        )*

        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Thresh($thresh, items))?
    });
    ( multi $thresh:expr, $keys:expr ) => ({
        $crate::miniscript::Miniscript::from_ast($crate::miniscript::miniscript::decode::Terminal::Multi($thresh, $keys))?
    });
}
