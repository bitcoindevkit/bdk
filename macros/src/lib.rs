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

#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

use syn::spanned::Spanned;
use syn::{parse, ImplItemMethod, ItemImpl, ItemTrait, Token};

fn add_async_trait(mut parsed: ItemTrait) -> TokenStream {
    let output = quote! {
        #[cfg(all(not(target_arch = "wasm32"), not(feature = "async-interface")))]
        #parsed
    };

    for mut item in &mut parsed.items {
        if let syn::TraitItem::Method(m) = &mut item {
            m.sig.asyncness = Some(Token![async](m.span()));
        }
    }

    let output = quote! {
        #output

        #[cfg(any(target_arch = "wasm32", feature = "async-interface"))]
        #[async_trait(?Send)]
        #parsed
    };

    output.into()
}

fn add_async_method(mut parsed: ImplItemMethod) -> TokenStream {
    let output = quote! {
        #[cfg(all(not(target_arch = "wasm32"), not(feature = "async-interface")))]
        #parsed
    };

    parsed.sig.asyncness = Some(Token![async](parsed.span()));

    let output = quote! {
        #output

        #[cfg(any(target_arch = "wasm32", feature = "async-interface"))]
        #parsed
    };

    output.into()
}

fn add_async_impl_trait(mut parsed: ItemImpl) -> TokenStream {
    let output = quote! {
        #[cfg(all(not(target_arch = "wasm32"), not(feature = "async-interface")))]
        #parsed
    };

    for mut item in &mut parsed.items {
        if let syn::ImplItem::Method(m) = &mut item {
            m.sig.asyncness = Some(Token![async](m.span()));
        }
    }

    let output = quote! {
        #output

        #[cfg(any(target_arch = "wasm32", feature = "async-interface"))]
        #[async_trait(?Send)]
        #parsed
    };

    output.into()
}

/// Makes a method or every method of a trait "async" only if the target_arch is "wasm32"
///
/// Requires the `async-trait` crate as a dependency whenever this attribute is used on a trait
/// definition or trait implementation.
#[proc_macro_attribute]
pub fn maybe_async(_attr: TokenStream, item: TokenStream) -> TokenStream {
    if let Ok(parsed) = parse(item.clone()) {
        add_async_trait(parsed)
    } else if let Ok(parsed) = parse(item.clone()) {
        add_async_method(parsed)
    } else if let Ok(parsed) = parse(item) {
        add_async_impl_trait(parsed)
    } else {
        (quote! {
            compile_error!("#[maybe_async] can only be used on methods, trait or trait impl blocks")
        })
        .into()
    }
}

/// Awaits if target_arch is "wasm32", does nothing otherwise
#[proc_macro]
pub fn maybe_await(expr: TokenStream) -> TokenStream {
    let expr: proc_macro2::TokenStream = expr.into();
    let quoted = quote! {
        {
            #[cfg(all(not(target_arch = "wasm32"), not(feature = "async-interface")))]
            {
                #expr
            }

            #[cfg(any(target_arch = "wasm32", feature = "async-interface"))]
            {
                #expr.await
            }
        }
    };

    quoted.into()
}

/// Awaits if target_arch is "wasm32", uses `tokio::Runtime::block_on()` otherwise
///
/// Requires the `tokio` crate as a dependecy with `rt-core` or `rt-threaded` to build on non-wasm32 platforms.
#[proc_macro]
pub fn await_or_block(expr: TokenStream) -> TokenStream {
    let expr: proc_macro2::TokenStream = expr.into();
    let quoted = quote! {
        {
            #[cfg(all(not(target_arch = "wasm32"), not(feature = "async-interface")))]
            {
                tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(#expr)
            }

            #[cfg(any(target_arch = "wasm32", feature = "async-interface"))]
            {
                #expr.await
            }
        }
    };

    quoted.into()
}
