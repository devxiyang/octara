use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::visit_mut::{self, VisitMut};
use syn::{Block, Expr, ExprBreak, Ident, Item, ItemFn, Lifetime, Pat, Token, Type, parse_quote};

pub(super) fn expand(mut function: ItemFn, has_args: bool) -> syn::Result<TokenStream> {
    let name = &function.sig.ident;
    if has_args {
        return Err(syn::Error::new_spanned(name, "`#[ui]` takes no arguments"));
    }
    if let Some(asynchronous) = function.sig.asyncness {
        return Err(syn::Error::new_spanned(asynchronous, "a UI component builds its view synchronously"));
    }
    if !function.attrs.iter().any(|attr| attr.path().is_ident("track_caller")) {
        function.attrs.push(parse_quote!(#[track_caller]));
    }
    // The body locals drop before provenance is attached.
    let body = component_body(*function.block);
    let result = Ident::new("__octara_result", Span::mixed_site());
    function.block = parse_quote!({
        let #result = #body;
        ::octara::view::IntoView::with_component(
            #result,
            ::octara::view::Component {
                name: ::core::stringify!(#name),
                module: ::core::module_path!(),
                at: ::core::panic::Location::caller(),
            },
        )
    });
    Ok(quote!(#function))
}

fn component_body(mut body: Block) -> TokenStream {
    let mut returns = RewriteReturns { label: Lifetime::new("'__octara_body", Span::mixed_site()), needs_label: false };
    returns.visit_block_mut(&mut body);
    if returns.needs_label {
        let label = returns.label;
        quote!(#label: #body)
    } else {
        quote!(#body)
    }
}

struct RewriteReturns {
    label: Lifetime,
    needs_label: bool,
}

impl VisitMut for RewriteReturns {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Return(ret) => {
                self.needs_label = true;
                visit_mut::visit_expr_return_mut(self, ret);
                *expr = Expr::Break(ExprBreak {
                    attrs: std::mem::take(&mut ret.attrs),
                    break_token: Token![break](ret.return_token.span),
                    label: Some(self.label.clone()),
                    expr: ret.expr.take(),
                });
            }
            Expr::Closure(_) | Expr::Async(_) | Expr::Const(_) => {}
            _ => visit_mut::visit_expr_mut(self, expr),
        }
    }

    // These positions cannot exit the enclosing function.
    fn visit_item_mut(&mut self, _: &mut Item) {}
    fn visit_pat_mut(&mut self, _: &mut Pat) {}
    fn visit_type_mut(&mut self, _: &mut Type) {}
}
