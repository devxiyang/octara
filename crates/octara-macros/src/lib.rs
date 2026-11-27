//! Octara component provenance, reactive stores, and compile-time style utilities.

mod store;
mod tw;

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, ItemFn, LitStr, parse_macro_input};

/// Compiles a string of whitespace-separated style utilities into an Octara `TwStyle`.
///
/// Layout, spacing, typography, radius, shadow, and color utilities map to authored style
/// properties. Numeric spacing uses theme steps, while `p-px` and `p-[13px]` use logical
/// pixels. Colors can use a semantic token such as `bg-surface` or a palette shade such
/// as `text-slate-900`. An opacity suffix like `bg-surface/80` sets alpha to 80 percent.
///
/// Prefix a utility with a condition such as `hover:`, `focus-within:`, `dark:`, or `md:`.
/// Use `group-hover:` with an ancestor declared as `.group("group")`. For an ancestor
/// declared as `.group("row")`, use `group-hover/row:`.
///
/// Unknown utilities and conditions are compile errors.
#[proc_macro]
pub fn tw(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as LitStr);
    tw::expand(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Marks an ordinary Rust view function so mounted nodes carry its component provenance.
///
/// A component that allocates state takes an explicit `&mut BuildCx<'_>` argument. Pure
/// components need no context. The function keeps its return type and runs synchronously.
#[proc_macro_attribute]
pub fn ui(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut function = parse_macro_input!(item as ItemFn);
    if !attr.is_empty() {
        return syn::Error::new_spanned(&function.sig.ident, "`#[ui]` takes no arguments").into_compile_error().into();
    }
    if let Some(asynchronous) = function.sig.asyncness {
        return syn::Error::new_spanned(asynchronous, "a UI component builds its view synchronously").into_compile_error().into();
    }
    if !function.attrs.iter().any(|attr| attr.path().is_ident("track_caller")) {
        function.attrs.push(syn::parse_quote!(#[track_caller]));
    }
    let name = &function.sig.ident;
    let body = function.block;
    function.block = syn::parse_quote!({
        let __octara_result = (|| #body)();
        ::octara::view::IntoView::with_component(
            __octara_result,
            ::octara::view::Component {
                name: ::core::stringify!(#name),
                module: ::core::module_path!(),
                at: ::core::panic::Location::caller(),
            },
        )
    });
    quote!(#function).into()
}

/// Generates a reactive accessor for every named field of a struct.
///
/// Create a store with `cx.store(value)`, then use `store.field()` with `r.with`,
/// `w.set`, or `w.mutate`. Each accessor has the field's name and visibility.
/// Annotate a field with `#[store(nested)]` to expose the accessors of another
/// derived store. Leaf fields need `PartialEq` for precise replacement.
#[proc_macro_derive(Store, attributes(store))]
pub fn store(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    store::expand(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}
