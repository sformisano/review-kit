//! Generates synchronous operation boundaries and private managed port facades.

mod declaration;
mod operations;
mod port;

use proc_macro::TokenStream;

/// Requires a disclosure policy or helper exemption for a function, inline module or impl.
#[proc_macro_attribute]
pub fn operations(arguments: TokenStream, item: TokenStream) -> TokenStream {
    operations::expand(arguments.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Generates the raw trait and a managed facade from one complete method contract.
#[proc_macro_attribute]
pub fn observed_port(arguments: TokenStream, item: TokenStream) -> TokenStream {
    port::expand(arguments.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
