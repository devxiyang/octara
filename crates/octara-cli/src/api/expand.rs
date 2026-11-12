//! A small `macro_rules!` expander: enough for the declarative macros the framework writes
//! families of items with (`steps!` in `Styled`, `codes!`), and for the two outside macros whose
//! items are API (`bitflags!`, slotmap's `new_key_type!`), so the API lists what they make.
//!
//! Rules are tried in order; fragments are matched by shape (`ident`, `literal`, `tt`, `block`;
//! `ty`, `expr`, `path`, `meta`, `vis` and the rest up to the token that follows them, angle
//! brackets kept together). What does not match is left out, not guessed.

use proc_macro2::{Delimiter, Group, Ident, Spacing, TokenStream, TokenTree};
use std::collections::HashMap;

/// One `macro_rules!` definition.
#[derive(Clone)]
pub struct Macro {
    rules: Vec<(Vec<Pat>, TokenStream)>,
}

#[derive(Clone, Debug)]
enum Pat {
    /// A token that must be there (compared as written).
    Tok(String),
    Group(Delimiter, Vec<Pat>),
    Var(String, String),
    Rep(Vec<Pat>, Option<String>, char),
}

#[derive(Clone, Debug)]
enum Bound {
    Leaf(Vec<TokenTree>, bool),
    Seq(Vec<Bindings>),
}

type Bindings = HashMap<String, Bound>;

/// The outside macros, as rules over their own input.
const BUILTIN: &[(&str, &str)] = &[
    (
        "bitflags",
        "($(#[$outer:meta])* $vis:vis struct $name:ident: $t:ty { $($(#[$inner:meta])* const $flag:ident = $value:expr;)* } $($rest:tt)*) => {
            $(#[$outer])* $vis struct $name($t);
            impl $name {
                $($(#[$inner])* pub const $flag: Self = Self($value);)*
                /// The set with no flags.
                pub const fn empty() -> Self { loop {} }
                /// The set with every flag.
                pub const fn all() -> Self { loop {} }
                /// The underlying bits.
                pub const fn bits(&self) -> $t { loop {} }
                /// The flags `bits` make, if every bit is a flag.
                pub const fn from_bits(bits: $t) -> Option<Self> { loop {} }
                /// The flags `bits` make, unknown bits dropped.
                pub const fn from_bits_truncate(bits: $t) -> Self { loop {} }
                /// The flags `bits` make, unknown bits kept.
                pub const fn from_bits_retain(bits: $t) -> Self { loop {} }
                /// The flag named `name`.
                pub fn from_name(name: &str) -> Option<Self> { loop {} }
                /// No flag is set.
                pub const fn is_empty(&self) -> bool { loop {} }
                /// Every flag is set.
                pub const fn is_all(&self) -> bool { loop {} }
                /// Any flag of `other` is set.
                pub const fn intersects(&self, other: Self) -> bool { loop {} }
                /// Every flag of `other` is set.
                pub const fn contains(&self, other: Self) -> bool { loop {} }
                /// Sets the flags of `other`.
                pub fn insert(&mut self, other: Self) { loop {} }
                /// Clears the flags of `other`.
                pub fn remove(&mut self, other: Self) { loop {} }
                /// Flips the flags of `other`.
                pub fn toggle(&mut self, other: Self) { loop {} }
                /// Sets or clears the flags of `other`.
                pub fn set(&mut self, other: Self, value: bool) { loop {} }
                /// The flags in both.
                pub const fn intersection(self, other: Self) -> Self { loop {} }
                /// The flags in either.
                pub const fn union(self, other: Self) -> Self { loop {} }
                /// The flags in this and not in `other`.
                pub const fn difference(self, other: Self) -> Self { loop {} }
                /// The flags in exactly one.
                pub const fn symmetric_difference(self, other: Self) -> Self { loop {} }
                /// The flags not in this.
                pub const fn complement(self) -> Self { loop {} }
                /// The flags set, one at a time.
                pub fn iter(&self) -> Iter<Self> { loop {} }
                /// The flags set with their names.
                pub fn iter_names(&self) -> IterNames<Self> { loop {} }
            }
            impl BitOr for $name {} impl BitOrAssign for $name {} impl BitAnd for $name {} impl BitAndAssign for $name {}
            impl BitXor for $name {} impl BitXorAssign for $name {} impl Sub for $name {} impl SubAssign for $name {}
            impl Not for $name {} impl Extend<$name> for $name {} impl FromIterator<$name> for $name {} impl IntoIterator for $name {}
            bitflags! { $($rest)* }
        };
        () => {};",
    ),
    ("new_key_type", "($($(#[$outer:meta])* $vis:vis struct $name:ident;)*) => { $($(#[$outer])* $vis struct $name(u64);)* };"),
];

/// The outside macro named `name`, if one is known.
pub fn builtin(name: &str) -> Option<Macro> {
    BUILTIN.iter().find(|(n, _)| *n == name).and_then(|(_, src)| Macro::parse(src.parse().ok()?))
}

impl Macro {
    /// A definition's body: `(matcher) => { transcriber };` rules.
    pub fn parse(body: TokenStream) -> Option<Macro> {
        let tts: Vec<TokenTree> = body.into_iter().collect();
        let mut rules = Vec::new();
        let mut i = 0;
        while i < tts.len() {
            let TokenTree::Group(m) = &tts[i] else { return None };
            match (tts.get(i + 1), tts.get(i + 2)) {
                (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b))) if a.as_char() == '=' && b.as_char() == '>' => {}
                _ => return None,
            }
            let TokenTree::Group(t) = tts.get(i + 3)? else { return None };
            rules.push((pattern(&m.stream().into_iter().collect::<Vec<_>>())?, t.stream()));
            i += 4;
            if matches!(tts.get(i), Some(TokenTree::Punct(p)) if p.as_char() == ';') {
                i += 1;
            }
        }
        Some(Macro { rules })
    }

    /// The expansion of `input` by the first rule that matches all of it.
    pub fn expand(&self, input: TokenStream) -> Option<TokenStream> {
        let input: Vec<TokenTree> = input.into_iter().collect();
        for (pats, out) in &self.rules {
            let mut b = Bindings::new();
            if match_seq(pats, &input, 0, &mut b, None) == Some(input.len()) {
                let mut ts = TokenStream::new();
                transcribe(&out.clone().into_iter().collect::<Vec<_>>(), &b, &mut Vec::new(), &mut ts)?;
                return Some(ts);
            }
        }
        None
    }
}

fn text(t: &TokenTree) -> String {
    match t {
        TokenTree::Group(g) => g.to_string(),
        other => other.to_string(),
    }
}

fn pattern(tts: &[TokenTree]) -> Option<Vec<Pat>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tts.len() {
        match &tts[i] {
            TokenTree::Punct(p) if p.as_char() == '$' => match tts.get(i + 1)? {
                TokenTree::Ident(name) => {
                    match (tts.get(i + 2), tts.get(i + 3)) {
                        (Some(TokenTree::Punct(c)), Some(TokenTree::Ident(frag))) if c.as_char() == ':' => {
                            out.push(Pat::Var(name.to_string(), frag.to_string()));
                            i += 4;
                        }
                        // `$crate` and friends in a matcher: as written.
                        _ => {
                            out.push(Pat::Tok(format!("${name}")));
                            i += 2;
                        }
                    }
                }
                TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner = pattern(&g.stream().into_iter().collect::<Vec<_>>())?;
                    let (sep, op, used) = repetition(&tts[i + 2..])?;
                    out.push(Pat::Rep(inner, sep, op));
                    i += 2 + used;
                }
                _ => return None,
            },
            TokenTree::Group(g) => {
                out.push(Pat::Group(g.delimiter(), pattern(&g.stream().into_iter().collect::<Vec<_>>())?));
                i += 1;
            }
            t => {
                out.push(Pat::Tok(text(t)));
                i += 1;
            }
        }
    }
    Some(out)
}

/// After `$( .. )`: an optional separator and the operator; how many tokens they took.
fn repetition(rest: &[TokenTree]) -> Option<(Option<String>, char, usize)> {
    let op = |t: &TokenTree| match t {
        TokenTree::Punct(p) if matches!(p.as_char(), '*' | '+' | '?') => Some(p.as_char()),
        _ => None,
    };
    let first = rest.first()?;
    if let Some(o) = op(first) {
        return Some((None, o, 1));
    }
    let o = op(rest.get(1)?)?;
    Some((Some(text(first)), o, 2))
}

/// Matches `pats` from `at`; where it stopped. `outer` is what follows `pats` where they are
/// used (a repetition's separator), for a fragment at their end.
fn match_seq(pats: &[Pat], input: &[TokenTree], mut at: usize, b: &mut Bindings, outer: Option<&Stop>) -> Option<usize> {
    for (k, p) in pats.iter().enumerate() {
        match p {
            Pat::Tok(s) => {
                if input.get(at).map(text).as_deref() != Some(s.as_str()) {
                    return None;
                }
                at += 1;
            }
            Pat::Group(d, inner) => {
                let TokenTree::Group(g) = input.get(at)? else { return None };
                if g.delimiter() != *d {
                    return None;
                }
                let tts: Vec<TokenTree> = g.stream().into_iter().collect();
                if match_seq(inner, &tts, 0, b, None)? != tts.len() {
                    return None;
                }
                at += 1;
            }
            Pat::Var(name, frag) => {
                let stop = follow(&pats[k + 1..]);
                let end = fragment(frag, input, at, stop.as_ref().or(outer))?;
                let wrap = matches!(frag.as_str(), "ty" | "expr" | "path" | "pat");
                b.insert(name.clone(), Bound::Leaf(input[at..end].to_vec(), wrap));
                at = end;
            }
            Pat::Rep(inner, sep, op) => {
                let after = follow(&pats[k + 1..]);
                let inner_stop = sep.clone().map(Stop::Tok).or(after).or_else(|| outer.map(Stop::copy));
                let mut iters = Vec::new();
                loop {
                    let mut ib = Bindings::new();
                    let Some(next) = match_seq(inner, input, at, &mut ib, inner_stop.as_ref()) else { break };
                    // An iteration that takes nothing would repeat forever.
                    if next == at {
                        break;
                    }
                    iters.push(ib);
                    at = next;
                    if *op == '?' {
                        break;
                    }
                    match sep {
                        Some(s) if input.get(at).map(text).as_deref() == Some(s.as_str()) => {
                            // A trailing separator ends the repetition when nothing follows it.
                            let mut probe = Bindings::new();
                            if match_seq(inner, input, at + 1, &mut probe, inner_stop.as_ref()).is_some() {
                                at += 1;
                            } else {
                                break;
                            }
                        }
                        Some(_) => break,
                        None => {
                            if at >= input.len() {
                                break;
                            }
                        }
                    }
                }
                if *op == '+' && iters.is_empty() {
                    return None;
                }
                for name in names(inner) {
                    b.insert(name, Bound::Seq(iters.clone()));
                }
            }
        }
    }
    Some(at)
}

/// What a fragment must stop before: a token, or a group of a delimiter.
enum Stop {
    Tok(String),
    Group(Delimiter),
}

impl Stop {
    fn copy(&self) -> Stop {
        match self {
            Stop::Tok(s) => Stop::Tok(s.clone()),
            Stop::Group(d) => Stop::Group(*d),
        }
    }
    fn at(&self, t: &TokenTree) -> bool {
        match (self, t) {
            (Stop::Group(d), TokenTree::Group(g)) => g.delimiter() == *d,
            (Stop::Tok(s), t) => text(t) == *s,
            _ => false,
        }
    }
}

/// What follows a fragment in its pattern.
fn follow(rest: &[Pat]) -> Option<Stop> {
    match rest.first()? {
        Pat::Tok(s) => Some(Stop::Tok(s.clone())),
        Pat::Group(d, _) => Some(Stop::Group(*d)),
        Pat::Rep(inner, _, _) => follow(inner),
        Pat::Var(..) => None,
    }
}

/// Where a fragment of kind `frag` starting at `at` ends.
fn fragment(frag: &str, input: &[TokenTree], at: usize, stop: Option<&Stop>) -> Option<usize> {
    let tok = |i: usize| input.get(i);
    match frag {
        "ident" => matches!(tok(at)?, TokenTree::Ident(_)).then_some(at + 1),
        "lifetime" => match (tok(at)?, tok(at + 1)?) {
            (TokenTree::Punct(p), TokenTree::Ident(_)) if p.as_char() == '\'' => Some(at + 2),
            _ => None,
        },
        "literal" => match tok(at)? {
            TokenTree::Literal(_) => Some(at + 1),
            TokenTree::Ident(i) if i == "true" || i == "false" => Some(at + 1),
            TokenTree::Punct(p) if p.as_char() == '-' && matches!(tok(at + 1), Some(TokenTree::Literal(_))) => Some(at + 2),
            _ => None,
        },
        "tt" => tok(at).map(|_| at + 1),
        "block" => matches!(tok(at)?, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace).then_some(at + 1),
        "vis" => match tok(at) {
            Some(TokenTree::Ident(i)) if i == "pub" => {
                Some(if matches!(tok(at + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) {
                    at + 2
                } else {
                    at + 1
                })
            }
            _ => Some(at),
        },
        // `ty`, `expr`, `path`, `pat`, `meta`, `item`, `stmt`: up to the token that follows, with
        // `<..>` kept whole.
        _ => {
            let mut i = at;
            let mut angle = 0i32;
            while let Some(t) = tok(i) {
                if angle == 0 && stop.is_some_and(|s| s.at(t)) {
                    break;
                }
                if let TokenTree::Punct(p) = t {
                    let joint_prev = i > at
                        && matches!(tok(i - 1), Some(TokenTree::Punct(q)) if q.spacing() == Spacing::Joint && matches!(q.as_char(), '-' | '='));
                    match p.as_char() {
                        '<' => angle += 1,
                        '>' if !joint_prev => angle -= 1,
                        _ => {}
                    }
                }
                i += 1;
            }
            (i > at).then_some(i)
        }
    }
}

fn names(pats: &[Pat]) -> Vec<String> {
    let mut out = Vec::new();
    for p in pats {
        match p {
            Pat::Var(n, _) => out.push(n.clone()),
            Pat::Group(_, inner) | Pat::Rep(inner, _, _) => out.extend(names(inner)),
            Pat::Tok(_) => {}
        }
    }
    out
}

/// The binding of `name` at the repetition `indices`.
fn lookup<'a>(b: &'a Bindings, name: &str, indices: &[usize]) -> Option<&'a Bound> {
    let mut cur = b.get(name)?;
    let mut depth = 0;
    while let Bound::Seq(items) = cur {
        let i = *indices.get(depth)?;
        cur = items.get(i)?.get(name)?;
        depth += 1;
    }
    Some(cur)
}

/// How many times a repetition at `indices` runs: the length of the first variable inside it
/// that repeats there.
fn count(tts: &[TokenTree], b: &Bindings, indices: &[usize]) -> Option<usize> {
    let mut i = 0;
    while i < tts.len() {
        match &tts[i] {
            TokenTree::Punct(p) if p.as_char() == '$' => {
                if let Some(TokenTree::Ident(n)) = tts.get(i + 1) {
                    let mut cur = b.get(&n.to_string());
                    let mut depth = 0;
                    while let Some(Bound::Seq(items)) = cur {
                        if depth == indices.len() {
                            return Some(items.len());
                        }
                        cur = items.get(indices[depth]).and_then(|m| m.get(&n.to_string()));
                        depth += 1;
                    }
                }
                if let Some(TokenTree::Group(g)) = tts.get(i + 1)
                    && let Some(n) = count(&g.stream().into_iter().collect::<Vec<_>>(), b, indices)
                {
                    return Some(n);
                }
            }
            TokenTree::Group(g) => {
                if let Some(n) = count(&g.stream().into_iter().collect::<Vec<_>>(), b, indices) {
                    return Some(n);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn transcribe(tts: &[TokenTree], b: &Bindings, indices: &mut Vec<usize>, out: &mut TokenStream) -> Option<()> {
    let mut i = 0;
    while i < tts.len() {
        match &tts[i] {
            TokenTree::Punct(p) if p.as_char() == '$' => match tts.get(i + 1) {
                Some(TokenTree::Ident(n)) if n == "crate" => {
                    out.extend([TokenTree::Ident(Ident::new("crate", n.span()))]);
                    i += 2;
                }
                Some(TokenTree::Ident(n)) => {
                    match lookup(b, &n.to_string(), indices)? {
                        Bound::Leaf(tokens, true) => {
                            out.extend([TokenTree::Group(Group::new(Delimiter::None, tokens.iter().cloned().collect()))]);
                        }
                        Bound::Leaf(tokens, false) => out.extend(tokens.iter().cloned()),
                        Bound::Seq(_) => return None,
                    }
                    i += 2;
                }
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                    let (sep, _, used) = repetition(&tts[i + 2..])?;
                    let n = count(&inner, b, indices).unwrap_or(0);
                    for k in 0..n {
                        if k > 0
                            && let Some(s) = &sep
                        {
                            out.extend(s.parse::<TokenStream>().ok()?);
                        }
                        indices.push(k);
                        transcribe(&inner, b, indices, out)?;
                        indices.pop();
                    }
                    i += 2 + used;
                }
                _ => {
                    out.extend([tts[i].clone()]);
                    i += 1;
                }
            },
            TokenTree::Group(g) => {
                let mut inner = TokenStream::new();
                transcribe(&g.stream().into_iter().collect::<Vec<_>>(), b, indices, &mut inner)?;
                let mut ng = Group::new(g.delimiter(), inner);
                ng.set_span(g.span());
                out.extend([TokenTree::Group(ng)]);
                i += 1;
            }
            t => {
                out.extend([t.clone()]);
                i += 1;
            }
        }
    }
    Some(())
}

/// The string a doc attribute's value makes: a literal, or `concat!` of literals and
/// `stringify!`s (what macros write their docs with).
pub fn doc_value(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
        syn::Expr::Group(g) => doc_value(&g.expr),
        syn::Expr::Macro(m) => macro_string(&m.mac),
        _ => None,
    }
}

fn macro_string(mac: &syn::Macro) -> Option<String> {
    let name = mac.path.segments.last()?.ident.to_string();
    match name.as_str() {
        "stringify" => Some(plain(mac.tokens.clone())),
        "concat" => {
            let parts = mac.parse_body_with(syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated).ok()?;
            let mut s = String::new();
            for p in parts {
                match &p {
                    syn::Expr::Lit(syn::ExprLit { lit, .. }) => s += &lit_text(lit)?,
                    other => s += &doc_value(other)?,
                }
            }
            Some(s)
        }
        _ => None,
    }
}

fn lit_text(lit: &syn::Lit) -> Option<String> {
    Some(match lit {
        syn::Lit::Str(s) => s.value(),
        syn::Lit::Int(i) => i.base10_digits().to_string(),
        syn::Lit::Float(f) => f.base10_digits().to_string(),
        syn::Lit::Bool(b) => b.value.to_string(),
        syn::Lit::Char(c) => c.value().to_string(),
        _ => return None,
    })
}

/// Tokens as `stringify!` writes them for the simple cases (idents and literals, None groups
/// opened).
fn plain(ts: TokenStream) -> String {
    let mut parts = Vec::new();
    for t in ts {
        match t {
            TokenTree::Group(g) if g.delimiter() == Delimiter::None => parts.push(plain(g.stream())),
            other => parts.push(other.to_string()),
        }
    }
    parts.join(" ")
}
