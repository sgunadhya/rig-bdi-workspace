use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, DeriveInput, Error, Ident, Result, parse_macro_input};

#[proc_macro_derive(Effectful, attributes(effect))]
pub fn derive_effectful(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match expand_effectful(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_effectful(input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let effect_attr = find_effect_attr(&input.attrs)?;
    let effect_expr = parse_effect_attr(effect_attr)?;

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics rig_effects::Effectful for #name #ty_generics #where_clause {
            fn effect(&self) -> rig_effects::Effect {
                #effect_expr
            }
        }
    })
}

fn find_effect_attr(attrs: &[Attribute]) -> Result<&Attribute> {
    attrs
        .iter()
        .find(|attr| attr.path().is_ident("effect"))
        .ok_or_else(|| Error::new(Span::call_site(), "missing #[effect(...)] attribute"))
}

fn parse_effect_attr(attr: &Attribute) -> Result<proc_macro2::TokenStream> {
    let spec = attr.parse_args::<EffectSpec>()?;

    Ok(match spec {
        EffectSpec::Pure => quote! { rig_effects::Effect::Pure },
        EffectSpec::Observe => quote! { rig_effects::Effect::Observe },
        EffectSpec::Mutate => quote! { rig_effects::Effect::Mutate },
        EffectSpec::Irreversible => quote! { rig_effects::Effect::Irreversible },
    })
}

enum EffectSpec {
    Pure,
    Observe,
    Mutate,
    Irreversible,
}

impl Parse for EffectSpec {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let effect: Ident = input.parse()?;

        let parsed = match effect.to_string().as_str() {
            "Pure" => Self::Pure,
            "Observe" => Self::Observe,
            "Mutate" => Self::Mutate,
            "Irreversible" => Self::Irreversible,
            other => {
                return Err(Error::new_spanned(
                    effect,
                    format!(
                        "unsupported effect `{other}`; expected Pure, Observe, Mutate, or Irreversible"
                    ),
                ))
            }
        };

        if input.is_empty() {
            Ok(parsed)
        } else {
            Err(input.error("unexpected tokens in #[effect(...)] attribute"))
        }
    }
}
