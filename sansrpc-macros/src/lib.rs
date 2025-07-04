use proc_macro::TokenStream;
use quote::ToTokens;
use syn::{ItemFn, ItemTrait, TraitItemFn};

mod service_impl;

macro_rules! parse_or_panic {
    ($expr:expr, $lit:literal) => {
        match ::syn::parse($expr) {
            Ok(t) => t,
            Err(_) => {
                panic!(
                    "{} attribute should be used inside a #[service] block on a trait fn",
                    $lit
                )
            }
        }
    };
}

#[proc_macro_attribute]
pub fn service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let t: ItemTrait = syn::parse(item.clone()).unwrap();
    item
}

#[proc_macro_attribute]
pub fn request(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let _t: TraitItemFn = parse_or_panic!(item.clone(), "#[request]");
    item
}

#[proc_macro_attribute]
pub fn streaming(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let _t: TraitItemFn = parse_or_panic!(item.clone(), "#[streaming]");
    item
}

#[proc_macro_attribute]
pub fn oneshot(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let t: TraitItemFn = parse_or_panic!(item.clone(), "#[oneshot]");

    item
}
