use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    Attribute, Expr, Ident, LitStr, Meta, ReturnType, Signature, Token, Type,
    parse::{Parse, ParseStream},
    spanned::Spanned,
};

pub(crate) struct Operation {
    pub(crate) name: LitStr,
    pub(crate) input: Expr,
    pub(crate) output: Expr,
    pub(crate) returns: Option<Type>,
    pub(crate) map: Option<Expr>,
    pub(crate) method: Option<Ident>,
}

impl Parse for Operation {
    fn parse(stream: ParseStream<'_>) -> syn::Result<Self> {
        let mut name = None;
        let mut input = None;
        let mut output = None;
        let mut returns = None;
        let mut map = None;
        let mut method = None;
        while !stream.is_empty() {
            let key: Ident = stream.parse()?;
            stream.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => set(&mut name, stream.parse::<LitStr>()?, &key)?,
                "input" => set(&mut input, stream.parse::<Expr>()?, &key)?,
                "output" => set(&mut output, stream.parse::<Expr>()?, &key)?,
                "returns" => set(&mut returns, stream.parse::<Type>()?, &key)?,
                "map" => set(&mut map, stream.parse::<Expr>()?, &key)?,
                "method" => set(&mut method, stream.parse::<Ident>()?, &key)?,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown operation policy field",
                    ));
                }
            }
            if !stream.is_empty() {
                stream.parse::<Token![,]>()?;
            }
        }
        let name = name.ok_or_else(|| stream.error("operation requires name = \"...\""))?;
        if name.value().trim().is_empty() {
            return Err(syn::Error::new(
                name.span(),
                "operation name must not be empty",
            ));
        }
        let input =
            input.ok_or_else(|| stream.error("operation requires input = <Fields expression>"))?;
        let output = output
            .ok_or_else(|| stream.error("operation requires output = <completion policy>"))?;
        if returns.is_some() != map.is_some() {
            return Err(stream.error("facade return mapping requires both returns and map"));
        }
        Ok(Self {
            name,
            input,
            output,
            returns,
            map,
            method,
        })
    }
}

fn set<T>(field: &mut Option<T>, value: T, key: &Ident) -> syn::Result<()> {
    if field.replace(value).is_some() {
        return Err(syn::Error::new(
            key.span(),
            "duplicate operation policy field",
        ));
    }
    Ok(())
}

pub(crate) enum Policy {
    Operation(Box<Operation>),
    Helper,
}

pub(crate) fn take_policy(
    attrs: &mut Vec<Attribute>,
    signature: &Signature,
    aliases_allowed: bool,
) -> syn::Result<(Policy, Vec<Operation>)> {
    let mut policy = None;
    let mut aliases = Vec::new();
    let mut retained = Vec::new();
    for attr in std::mem::take(attrs) {
        if attr.path().is_ident("operation") || attr.path().is_ident("helper") {
            if policy.is_some() {
                return Err(syn::Error::new(
                    attr.span(),
                    "each function requires exactly one operation or helper declaration",
                ));
            }
            policy = Some(if attr.path().is_ident("helper") {
                if !matches!(attr.meta, Meta::Path(_)) {
                    return Err(syn::Error::new(attr.span(), "helper takes no arguments"));
                }
                Policy::Helper
            } else {
                let operation: Operation = attr.parse_args()?;
                if operation.method.is_some() {
                    return Err(syn::Error::new(
                        attr.span(),
                        "method is only valid in operation_alias",
                    ));
                }
                Policy::Operation(Box::new(operation))
            });
        } else if attr.path().is_ident("operation_alias") {
            if !aliases_allowed {
                return Err(syn::Error::new(
                    attr.span(),
                    "operation_alias requires observed_port",
                ));
            }
            let alias: Operation = attr.parse_args()?;
            if alias.method.is_none() {
                return Err(syn::Error::new(
                    attr.span(),
                    "operation_alias requires method = <name>",
                ));
            }
            aliases.push(alias);
        } else {
            retained.push(attr);
        }
    }
    *attrs = retained;
    let policy = policy.ok_or_else(|| {
        syn::Error::new(
            signature.ident.span(),
            "managed functions require #[operation(...)] or #[helper]",
        )
    })?;
    if matches!(policy, Policy::Operation(_)) {
        for attr in attrs.iter() {
            if attr.path().is_ident("track_caller") {
                return Err(syn::Error::new(
                    attr.span(),
                    "observed operations do not support track_caller; the generated body is a closure",
                ));
            }
            if attr.path().is_ident("cfg_attr") {
                return Err(syn::Error::new(
                    attr.span(),
                    "observed operations do not support cfg_attr; declare supported attributes directly",
                ));
            }
        }
    }
    if matches!(policy, Policy::Helper) && !aliases.is_empty() {
        return Err(syn::Error::new(
            signature.ident.span(),
            "helper methods cannot declare operation aliases",
        ));
    }
    Ok((policy, aliases))
}

pub(crate) fn validate_signature(signature: &Signature) -> syn::Result<()> {
    if signature.asyncness.is_some()
        || signature.constness.is_some()
        || signature.unsafety.is_some()
        || signature.abi.is_some()
        || signature.variadic.is_some()
    {
        return Err(syn::Error::new(
            signature.span(),
            "managed functions must be synchronous, non-const, safe Rust functions without variadic arguments",
        ));
    }
    Ok(())
}

pub(crate) fn body(operation: &Operation, body: TokenStream) -> TokenStream {
    let Operation {
        name,
        input,
        output,
        ..
    } = operation;
    let input_binding = Ident::new("__review_kit_input", Span::mixed_site());
    let result_binding = Ident::new("__review_kit_result", Span::mixed_site());
    quote! {{
        let #input_binding = ::review_kit_observation::__private::project_input(|| #input);
        ::review_kit_observation::__private::execute(
            #name,
            #input_binding,
            || { #body },
            |#result_binding| ::review_kit_observation::__private::map_value(#result_binding, #output),
        )
    }}
}

pub(crate) fn mapped_output(signature: &mut Signature, operation: &Operation) {
    if let Some(returns) = &operation.returns {
        signature.output = ReturnType::Type(Default::default(), Box::new(returns.clone()));
    }
}
