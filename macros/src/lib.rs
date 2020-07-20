#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

use syn::spanned::Spanned;
use syn::{parse, ImplItemMethod, ItemImpl, ItemTrait, Token};

fn add_async_trait(mut parsed: ItemTrait) -> TokenStream {
    let output = quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #parsed
    };

    for mut item in &mut parsed.items {
        if let syn::TraitItem::Method(m) = &mut item {
            m.sig.asyncness = Some(Token![async](m.span()));
        }
    }

    let output = quote! {
        #output

        #[cfg(target_arch = "wasm32")]
        #[async_trait(?Send)]
        #parsed
    };

    output.into()
}

fn add_async_method(mut parsed: ImplItemMethod) -> TokenStream {
    let output = quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #parsed
    };

    parsed.sig.asyncness = Some(Token![async](parsed.span()));

    let output = quote! {
        #output

        #[cfg(target_arch = "wasm32")]
        #parsed
    };

    output.into()
}

fn add_async_impl_trait(mut parsed: ItemImpl) -> TokenStream {
    let output = quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #parsed
    };

    for mut item in &mut parsed.items {
        if let syn::ImplItem::Method(m) = &mut item {
            m.sig.asyncness = Some(Token![async](m.span()));
        }
    }

    let output = quote! {
        #output

        #[cfg(target_arch = "wasm32")]
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
        }).into()
    }
}

/// Awaits if target_arch is "wasm32", does nothing otherwise
#[proc_macro]
pub fn maybe_await(expr: TokenStream) -> TokenStream {
    let expr: proc_macro2::TokenStream = expr.into();
    let quoted = quote! {
        {
            #[cfg(not(target_arch = "wasm32"))]
            {
                #expr
            }

            #[cfg(target_arch = "wasm32")]
            {
                #expr.await
            }
        }
    };

    quoted.into()
}

/// Awaits if target_arch is "wasm32", uses `futures::executor::block_on()` otherwise
///
/// Requires the `tokio` crate as a dependecy with `rt-core` or `rt-threaded` to build on non-wasm32 platforms.
#[proc_macro]
pub fn await_or_block(expr: TokenStream) -> TokenStream {
    let expr: proc_macro2::TokenStream = expr.into();
    let quoted = quote! {
        {
            #[cfg(not(target_arch = "wasm32"))]
            {
                tokio::runtime::Runtime::new().unwrap().block_on(#expr)
            }

            #[cfg(target_arch = "wasm32")]
            {
                #expr.await
            }
        }
    };

    quoted.into()
}
