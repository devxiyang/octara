//! Octara's attribute macros, re-exported by `octara` (`octara::prelude::component`).
//!
//! Written on the token stream alone (no parser crate): an attribute on a function needs only
//! its name and its body.

use proc_macro::{Delimiter, Group, Ident, Punct, Spacing, Span, TokenStream, TokenTree};

/// Marks a function as a component: the elements it builds (and those its views build later)
/// record it, with where it was called, so the inspector, picking and marked captures can say
/// "made by `toolbar()`, called at `src/app.rs:30:5`". Nothing is allocated, and state scopes
/// stay `component(name, ..)`'s.
///
/// Each element's own source stays the line in the body that built it. A component that is
/// also `#[track_caller]` is a leaf whose insides are not its caller's to edit (a library's
/// button): its elements' sources are where it was called, as `#[track_caller]` means.
///
/// ```ignore
/// #[component]
/// fn toolbar(save: impl Fn() + 'static) -> El {
///     row().gap_2().child(primary_button("Save").id("save").on_press(save))
/// }
/// ```
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return error(Span::call_site(), "`#[component]` takes no arguments");
    }
    let mut tokens: Vec<TokenTree> = item.into_iter().collect();
    let Some(at) = tokens.iter().position(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "fn")) else {
        return error(Span::call_site(), "`#[component]` goes on a function");
    };
    if tokens[..at].iter().any(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "async")) {
        return error(Span::call_site(), "a component builds its view at once: it cannot be `async`");
    }
    let Some(TokenTree::Ident(name)) = tokens.get(at + 1).cloned() else {
        return error(Span::call_site(), "`#[component]` goes on a named function");
    };
    let Some(TokenTree::Group(body)) =
        tokens.last().cloned().filter(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace))
    else {
        return error(name.span(), "`#[component]` needs the function's body");
    };
    let leaf = tokens[..at].iter().any(
        |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Bracket && g.stream().to_string().trim() == "track_caller"),
    );
    let guard: TokenStream = format!(
        "let __octara_component = ::octara::view::enter_component({:?}, ::core::module_path!(), ::core::panic::Location::caller());",
        name.to_string()
    )
    .parse()
    .expect("the guard parses");
    let mut inner = guard;
    if leaf {
        inner.extend(body.stream());
    } else {
        // The body runs in a closure: `#[track_caller]` does not reach through one, so the
        // elements it builds keep their own lines.
        let mut closure: TokenStream = "||".parse().expect("a closure head");
        closure.extend([TokenTree::Group(body.clone())]);
        inner.extend([
            TokenTree::Group(Group::new(Delimiter::Parenthesis, closure)),
            TokenTree::Group(Group::new(Delimiter::Parenthesis, TokenStream::new())),
        ]);
    }
    let mut new_body = Group::new(Delimiter::Brace, inner);
    new_body.set_span(body.span());
    *tokens.last_mut().expect("a body") = TokenTree::Group(new_body);

    let mut out = TokenStream::new();
    if !leaf {
        out.extend("#[track_caller]".parse::<TokenStream>().expect("the attribute parses"));
    }
    out.extend(tokens);
    out
}

fn error(span: Span, message: &str) -> TokenStream {
    let mut out = TokenStream::new();
    out.extend([
        TokenTree::Punct(Punct::new(':', Spacing::Joint)),
        TokenTree::Punct(Punct::new(':', Spacing::Alone)),
        TokenTree::Ident(Ident::new("core", span)),
        TokenTree::Punct(Punct::new(':', Spacing::Joint)),
        TokenTree::Punct(Punct::new(':', Spacing::Alone)),
        TokenTree::Ident(Ident::new("compile_error", span)),
        TokenTree::Punct(Punct::new('!', Spacing::Alone)),
    ]);
    let mut lit: TokenStream = format!("{message:?}").parse().expect("a string literal");
    lit = lit
        .into_iter()
        .map(|mut t| {
            t.set_span(span);
            t
        })
        .collect();
    out.extend([TokenTree::Group(Group::new(Delimiter::Parenthesis, lit))]);
    out.extend(["; ".parse::<TokenStream>().expect("a semicolon")]);
    out
}
