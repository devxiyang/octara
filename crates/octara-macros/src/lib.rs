//! Octara component provenance, reactive stores, and compile-time style utilities.

mod store;
mod tw;
mod ui;

use proc_macro::TokenStream;
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
    let function = parse_macro_input!(item as ItemFn);
    ui::expand(function, !attr.is_empty()).unwrap_or_else(syn::Error::into_compile_error).into()
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
