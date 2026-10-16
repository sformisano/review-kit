use proc_macro2::TokenStream;
use quote::quote;
use syn::{ImplItem, Item, ItemFn, ItemImpl, ItemMod, spanned::Spanned};

use crate::declaration::{self, Policy};

pub(crate) fn expand(arguments: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !arguments.is_empty() {
        return Err(syn::Error::new_spanned(
            arguments,
            "operations takes no arguments",
        ));
    }
    let mut item: Item = syn::parse2(item)?;
    match &mut item {
        Item::Fn(function) => function_item(function)?,
        Item::Mod(module) => module_items(module)?,
        Item::Impl(implementation) => impl_items(implementation)?,
        other => {
            return Err(syn::Error::new(
                other.span(),
                "operations requires a function, inline module or impl block",
            ));
        }
    }
    Ok(quote!(#item))
}

fn function_item(function: &mut ItemFn) -> syn::Result<()> {
    declaration::validate_signature(&function.sig)?;
    let (policy, _) = declaration::take_policy(&mut function.attrs, &function.sig, false)?;
    if let Policy::Operation(operation) = policy {
        reject_mapping(&operation)?;
        let original = &function.block;
        function.block = syn::parse2(declaration::body(&operation, quote!(#original)))?;
    }
    Ok(())
}

fn module_items(module: &mut ItemMod) -> syn::Result<()> {
    let (_, items) = module.content.as_mut().ok_or_else(|| syn::Error::new(module.ident.span(), "operations requires an inline module; out-of-line modules need their own managed declaration"))?;
    for item in items {
        match item {
            Item::Fn(function) => function_item(function)?,
            Item::Impl(implementation) => impl_items(implementation)?,
            Item::Mod(module) => module_items(module)?,
            Item::Trait(_) | Item::Macro(_) | Item::ForeignMod(_) => {
                return Err(syn::Error::new(
                    item.span(),
                    "managed modules cannot contain trait, macro, or foreign declarations",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn impl_items(implementation: &mut ItemImpl) -> syn::Result<()> {
    for item in &mut implementation.items {
        match item {
            ImplItem::Fn(function) => {
                declaration::validate_signature(&function.sig)?;
                let (policy, _) =
                    declaration::take_policy(&mut function.attrs, &function.sig, false)?;
                if let Policy::Operation(operation) = policy {
                    reject_mapping(&operation)?;
                    let original = &function.block;
                    function.block = syn::parse2(declaration::body(&operation, quote!(#original)))?;
                }
            }
            ImplItem::Macro(_) => {
                return Err(syn::Error::new(
                    item.span(),
                    "managed impls cannot contain item macros",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn reject_mapping(operation: &declaration::Operation) -> syn::Result<()> {
    if operation.returns.is_some() || operation.map.is_some() {
        return Err(syn::Error::new(
            operation.name.span(),
            "return mapping is only supported on observed_port methods",
        ));
    }
    Ok(())
}
