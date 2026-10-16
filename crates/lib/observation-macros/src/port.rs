use std::collections::BTreeSet;

use proc_macro2::{TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::{
    Expr, FnArg, Ident, ItemTrait, LitStr, Pat, Path, Signature, Token, TraitItem, Visibility,
    parse::{Parse, ParseStream},
    spanned::Spanned,
};

use crate::declaration::{self, Policy};

enum Ownership {
    Borrowed,
    Boxed,
}

struct Options {
    wrapper: Ident,
    ownership: Ownership,
    reader: Option<Path>,
    visibility: Option<Visibility>,
}

impl Parse for Options {
    fn parse(stream: ParseStream<'_>) -> syn::Result<Self> {
        let mut wrapper = None;
        let mut ownership = None;
        let mut reader = None;
        let mut visibility = None;
        let mut keys = BTreeSet::new();
        while !stream.is_empty() {
            let key: Ident = stream.parse()?;
            if !keys.insert(key.to_string()) {
                return Err(syn::Error::new(
                    key.span(),
                    "duplicate observed_port option",
                ));
            }
            stream.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "wrapper" => wrapper = Some(stream.parse()?),
                "ownership" => {
                    let value: LitStr = stream.parse()?;
                    ownership = Some(match value.value().as_str() {
                        "borrowed" => Ownership::Borrowed,
                        "boxed" => Ownership::Boxed,
                        _ => {
                            return Err(syn::Error::new(
                                value.span(),
                                "ownership must be borrowed or boxed",
                            ));
                        }
                    });
                }
                "reader" => reader = Some(stream.parse()?),
                "visibility" => {
                    let value: LitStr = stream.parse()?;
                    visibility = Some(value.parse()?);
                }
                _ => return Err(syn::Error::new(key.span(), "unknown observed_port option")),
            }
            if !stream.is_empty() {
                stream.parse::<Token![,]>()?;
            }
        }
        Ok(Self {
            wrapper: wrapper
                .ok_or_else(|| stream.error("observed_port requires wrapper = <name>"))?,
            ownership: ownership.ok_or_else(|| {
                stream.error("observed_port requires ownership = \"borrowed\" or \"boxed\"")
            })?,
            reader,
            visibility,
        })
    }
}

pub(crate) fn expand(arguments: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let options: Options = syn::parse2(arguments)?;
    if let Some(reader) = &options.reader {
        reject_parent_paths(reader)?;
    }
    let mut contract: ItemTrait = syn::parse2(item)?;
    if contract.unsafety.is_some()
        || contract.auto_token.is_some()
        || !contract.generics.params.is_empty()
        || contract.generics.where_clause.is_some()
    {
        return Err(syn::Error::new(
            contract.ident.span(),
            "observed_port requires a safe, non-generic trait",
        ));
    }
    if !contract.supertraits.is_empty()
        && (options.reader.is_none() || contract.supertraits.len() != 1)
    {
        return Err(syn::Error::new(
            contract.supertraits.span(),
            "a port supertrait requires one managed reader view",
        ));
    }
    if options.reader.is_some()
        && (contract.supertraits.is_empty() || !matches!(options.ownership, Ownership::Boxed))
    {
        return Err(syn::Error::new(
            contract.ident.span(),
            "reader requires boxed ownership and one reader supertrait",
        ));
    }
    let mut methods = Vec::new();
    let mut names = BTreeSet::from(["new".to_owned()]);
    if options.reader.is_some() {
        names.insert("reader".to_owned());
    }
    for item in &mut contract.items {
        let TraitItem::Fn(method) = item else {
            return Err(syn::Error::new(
                item.span(),
                "observed_port supports method declarations only",
            ));
        };
        reject_parent_paths(&method.sig)?;
        for attribute in &method.attrs {
            reject_parent_paths(attribute)?;
        }
        declaration::validate_signature(&method.sig)?;
        validate_method(&method.sig)?;
        let (policy, aliases) = declaration::take_policy(&mut method.attrs, &method.sig, true)?;
        add_name(&mut names, &method.sig.ident)?;
        methods.push(forward(&method.sig, &method.attrs, &policy, None)?);
        for alias in aliases {
            let alias_name = alias.method.as_ref().ok_or_else(|| {
                syn::Error::new(method.sig.span(), "alias method name is missing")
            })?;
            add_name(&mut names, alias_name)?;
            methods.push(forward(
                &method.sig,
                &method.attrs,
                &Policy::Operation(Box::new(alias)),
                Some(&method.sig.ident),
            )?);
        }
    }
    let trait_name = &contract.ident;
    let wrapper = &options.wrapper;
    let visibility = options.visibility.as_ref().unwrap_or(&contract.vis);
    let module = format_ident!(
        "__review_kit_port_{}",
        trait_name.to_string().to_lowercase()
    );
    let inner = match options.ownership {
        Ownership::Borrowed => quote!(&'a mut (dyn #trait_name + 'a)),
        Ownership::Boxed => quote!(::std::boxed::Box<dyn #trait_name + 'a>),
    };
    let reader = options.reader.map(|reader| {
        quote! {
            pub fn reader(&mut self) -> #reader<'_> {
                #reader::new(&mut *self.inner)
            }
        }
    });
    let conditional: Vec<_> = contract
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();
    if let Some(attr) = contract
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("cfg_attr"))
    {
        return Err(syn::Error::new(
            attr.span(),
            "observed_port does not support cfg_attr on traits",
        ));
    }
    Ok(quote! {
        #contract
        #(#conditional)*
        mod #module {
            use super::*;

            pub struct #wrapper<'a> { inner: #inner }

            impl<'a> #wrapper<'a> {
                pub fn new(inner: #inner) -> Self { Self { inner } }
                #(#methods)*
                #reader
            }
        }
        #(#conditional)*
        #visibility use #module::#wrapper;
    })
}

fn reject_parent_paths(value: &impl ToTokens) -> syn::Result<()> {
    let tokens: Vec<_> = value.to_token_stream().into_iter().collect();
    for window in tokens.windows(3) {
        if let [
            TokenTree::Ident(name),
            TokenTree::Punct(first),
            TokenTree::Punct(second),
        ] = window
            && name == "super"
            && first.as_char() == ':'
            && second.as_char() == ':'
        {
            return Err(syn::Error::new(
                name.span(),
                "observed_port moves facade declarations into a child module; use imported or crate:: paths instead of super::",
            ));
        }
    }
    for token in tokens {
        if let TokenTree::Group(group) = token {
            reject_parent_paths(&group.stream())?;
        }
    }
    Ok(())
}

fn add_name(names: &mut BTreeSet<String>, name: &Ident) -> syn::Result<()> {
    if !names.insert(name.to_string()) {
        return Err(syn::Error::new(
            name.span(),
            "duplicate or reserved facade method name",
        ));
    }
    Ok(())
}

fn validate_method(signature: &Signature) -> syn::Result<()> {
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            signature.generics.span(),
            "observed_port does not support generic methods",
        ));
    }
    let Some(FnArg::Receiver(receiver)) = signature.inputs.first() else {
        return Err(syn::Error::new(
            signature.span(),
            "observed_port methods require &self or &mut self",
        ));
    };
    if receiver.reference.is_none() || receiver.colon_token.is_some() {
        return Err(syn::Error::new(
            receiver.span(),
            "observed_port methods require &self or &mut self",
        ));
    }
    for argument in signature.inputs.iter().skip(1) {
        if let FnArg::Typed(argument) = argument {
            let Pat::Ident(pattern) = argument.pat.as_ref() else {
                return Err(syn::Error::new(
                    argument.pat.span(),
                    "observed_port arguments require plain identifiers",
                ));
            };
            if pattern.by_ref.is_some() || pattern.subpat.is_some() {
                return Err(syn::Error::new(
                    pattern.span(),
                    "observed_port arguments require plain identifiers",
                ));
            }
        }
    }
    Ok(())
}

fn forward(
    original: &Signature,
    attrs: &[syn::Attribute],
    policy: &Policy,
    alias_target: Option<&Ident>,
) -> syn::Result<TokenStream> {
    let mut signature = original.clone();
    let target = alias_target.unwrap_or(&original.ident);
    let mut args = Vec::new();
    for argument in signature.inputs.iter_mut().skip(1) {
        if let FnArg::Typed(argument) = argument
            && let Pat::Ident(pattern) = argument.pat.as_mut()
        {
            pattern.mutability = None;
            args.push(pattern.ident.clone());
        }
    }
    let call = quote!(self.inner.#target(#(#args),*));
    let body = match policy {
        Policy::Helper => quote!({ #call }),
        Policy::Operation(operation) => {
            if let Some(name) = &operation.method {
                signature.ident = name.clone();
            }
            declaration::mapped_output(&mut signature, operation);
            let call = match &operation.map {
                Some(map) => mapped(call, map),
                None => call,
            };
            declaration::body(operation, call)
        }
    };
    Ok(quote!(#(#attrs)* pub #signature #body))
}

fn mapped(call: TokenStream, map: &Expr) -> TokenStream {
    quote!(::review_kit_observation::__private::map_value(#call, #map))
}
