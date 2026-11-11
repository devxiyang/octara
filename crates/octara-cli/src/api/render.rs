//! Rendering rustdoc's structured types back into Rust source text, the way docs.rs shows them:
//! paths by their last segment, `impl Trait` arguments inline, `self` receivers short.

use rustdoc_types::{
    Abi, AssocItemConstraint, AssocItemConstraintKind, FunctionHeader, FunctionSignature, GenericArg, GenericArgs, GenericBound,
    GenericParamDef, GenericParamDefKind, Generics, Path, PreciseCapturingArg, Term, TraitBoundModifier, Type, WherePredicate,
};

pub fn ty(t: &Type) -> String {
    match t {
        Type::ResolvedPath(p) => path(p),
        Type::DynTrait(d) => {
            let mut parts: Vec<String> = d.traits.iter().map(|p| hrtb(&p.generic_params) + &path(&p.trait_)).collect();
            if let Some(l) = &d.lifetime {
                parts.push(l.clone());
            }
            format!("dyn {}", parts.join(" + "))
        }
        Type::Generic(s) | Type::Primitive(s) => s.clone(),
        Type::FunctionPointer(f) => {
            let mut s = hrtb(&f.generic_params);
            s += &header(&f.header);
            s += "fn(";
            s += &f.sig.inputs.iter().map(|(_, t)| ty(t)).collect::<Vec<_>>().join(", ");
            s += ")";
            s += &output(&f.sig);
            s
        }
        Type::Tuple(ts) if ts.len() == 1 => format!("({},)", ty(&ts[0])),
        Type::Tuple(ts) => format!("({})", ts.iter().map(ty).collect::<Vec<_>>().join(", ")),
        Type::Slice(t) => format!("[{}]", ty(t)),
        Type::Array { type_, len } => format!("[{}; {len}]", ty(type_)),
        Type::Pat { type_, .. } => ty(type_),
        Type::ImplTrait(bounds) => format!("impl {}", self::bounds(bounds)),
        Type::Infer => "_".into(),
        Type::RawPointer { is_mutable, type_ } => format!("*{} {}", if *is_mutable { "mut" } else { "const" }, ty(type_)),
        Type::BorrowedRef { lifetime, is_mutable, type_ } => {
            let mut s = "&".to_string();
            if let Some(l) = lifetime {
                s += l;
                s += " ";
            }
            if *is_mutable {
                s += "mut ";
            }
            s + &ty(type_)
        }
        Type::QualifiedPath { name, args, self_type, trait_ } => {
            let args = args.as_deref().map(generic_args).unwrap_or_default();
            match (self_type.as_ref(), trait_) {
                (Type::Generic(g), _) => format!("{g}::{name}{args}"),
                (s, Some(t)) => format!("<{} as {}>::{name}{args}", ty(s), path(t)),
                (s, None) => format!("{}::{name}{args}", ty(s)),
            }
        }
    }
}

/// A path by its last segment, with its generic arguments.
pub fn path(p: &Path) -> String {
    let name = p.path.rsplit("::").next().unwrap_or(&p.path);
    name.to_string() + &p.args.as_deref().map(generic_args).unwrap_or_default()
}

fn generic_args(a: &GenericArgs) -> String {
    match a {
        GenericArgs::AngleBracketed { args, constraints } => {
            let mut parts: Vec<String> = args.iter().map(generic_arg).collect();
            parts.extend(constraints.iter().map(constraint));
            if parts.is_empty() { String::new() } else { format!("<{}>", parts.join(", ")) }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let mut s = format!("({})", inputs.iter().map(ty).collect::<Vec<_>>().join(", "));
            if let Some(o) = output {
                s += " -> ";
                s += &ty(o);
            }
            s
        }
        GenericArgs::ReturnTypeNotation => "(..)".into(),
    }
}

fn generic_arg(a: &GenericArg) -> String {
    match a {
        GenericArg::Lifetime(l) => l.clone(),
        GenericArg::Type(t) => ty(t),
        GenericArg::Const(c) => c.expr.clone(),
        GenericArg::Infer => "_".into(),
    }
}

fn constraint(c: &AssocItemConstraint) -> String {
    let args = c.args.as_deref().map(generic_args).unwrap_or_default();
    match &c.binding {
        AssocItemConstraintKind::Equality(t) => format!("{}{args} = {}", c.name, term(t)),
        AssocItemConstraintKind::Constraint(b) => format!("{}{args}: {}", c.name, bounds(b)),
    }
}

fn term(t: &Term) -> String {
    match t {
        Term::Type(t) => ty(t),
        Term::Constant(c) => c.expr.clone(),
    }
}

pub fn bounds(bs: &[GenericBound]) -> String {
    bs.iter().map(bound).collect::<Vec<_>>().join(" + ")
}

fn bound(b: &GenericBound) -> String {
    match b {
        GenericBound::TraitBound { trait_, generic_params, modifier } => {
            let m = match modifier {
                TraitBoundModifier::None => "",
                TraitBoundModifier::Maybe => "?",
                TraitBoundModifier::MaybeConst => "~const ",
            };
            format!("{}{m}{}", hrtb(generic_params), path(trait_))
        }
        GenericBound::Outlives(l) => l.clone(),
        GenericBound::Use(args) => format!(
            "use<{}>",
            args.iter()
                .map(|a| match a {
                    PreciseCapturingArg::Lifetime(l) | PreciseCapturingArg::Param(l) => l.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn hrtb(params: &[GenericParamDef]) -> String {
    if params.is_empty() { String::new() } else { format!("for<{}> ", params.iter().map(param).collect::<Vec<_>>().join(", ")) }
}

fn param(p: &GenericParamDef) -> String {
    match &p.kind {
        GenericParamDefKind::Lifetime { outlives } if outlives.is_empty() => p.name.clone(),
        GenericParamDefKind::Lifetime { outlives } => format!("{}: {}", p.name, outlives.join(" + ")),
        GenericParamDefKind::Type { bounds: b, default, .. } => {
            let mut s = p.name.clone();
            if !b.is_empty() {
                s += ": ";
                s += &bounds(b);
            }
            if let Some(d) = default {
                s += " = ";
                s += &ty(d);
            }
            s
        }
        GenericParamDefKind::Const { type_, default } => {
            let mut s = format!("const {}: {}", p.name, ty(type_));
            if let Some(d) = default {
                s += " = ";
                s += d;
            }
            s
        }
    }
}

/// `<T: Clone, 'a>`, leaving out the parameters `impl Trait` arguments introduce.
pub fn generics(g: &Generics) -> String {
    let ps: Vec<String> =
        g.params.iter().filter(|p| !matches!(p.kind, GenericParamDefKind::Type { is_synthetic: true, .. })).map(param).collect();
    if ps.is_empty() { String::new() } else { format!("<{}>", ps.join(", ")) }
}

/// ` where T: Clone`, or nothing.
pub fn where_clause(g: &Generics) -> String {
    if g.where_predicates.is_empty() {
        return String::new();
    }
    let preds: Vec<String> = g
        .where_predicates
        .iter()
        .map(|w| match w {
            WherePredicate::BoundPredicate { type_, bounds: b, generic_params } => {
                format!("{}{}: {}", hrtb(generic_params), ty(type_), bounds(b))
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => format!("{lifetime}: {}", outlives.join(" + ")),
            WherePredicate::EqPredicate { lhs, rhs } => format!("{} = {}", ty(lhs), term(rhs)),
        })
        .collect();
    format!(" where {}", preds.join(", "))
}

pub fn header(h: &FunctionHeader) -> String {
    let mut s = String::new();
    if h.is_const {
        s += "const ";
    }
    if h.is_async {
        s += "async ";
    }
    if h.is_unsafe {
        s += "unsafe ";
    }
    match &h.abi {
        Abi::Rust => {}
        Abi::C { .. } => s += "extern \"C\" ",
        Abi::System { .. } => s += "extern \"system\" ",
        Abi::Other(o) => s += &format!("extern \"{o}\" "),
        _ => s += "extern ",
    }
    s
}

fn output(sig: &FunctionSignature) -> String {
    match &sig.output {
        None => String::new(),
        Some(Type::Tuple(t)) if t.is_empty() => String::new(),
        Some(t) => format!(" -> {}", ty(t)),
    }
}

/// `fn name<G>(args) -> out where ..`, receivers written `self`, `&self`, `&mut self`.
pub fn function(name: &str, f: &rustdoc_types::Function) -> String {
    let inputs: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|(n, t)| {
            if n == "self" {
                match t {
                    Type::Generic(s) if s == "Self" => return "self".into(),
                    Type::BorrowedRef { lifetime, is_mutable, type_ } if matches!(type_.as_ref(), Type::Generic(s) if s == "Self") => {
                        let l = lifetime.as_ref().map(|l| format!("{l} ")).unwrap_or_default();
                        return format!("&{l}{}self", if *is_mutable { "mut " } else { "" });
                    }
                    _ => {}
                }
            }
            format!("{n}: {}", ty(t))
        })
        .collect();
    format!("{}fn {name}{}({}){}{}", header(&f.header), generics(&f.generics), inputs.join(", "), output(&f.sig), where_clause(&f.generics))
}

/// Whether the function takes `self` in any form.
pub fn has_receiver(f: &rustdoc_types::Function) -> bool {
    f.sig.inputs.first().is_some_and(|(n, _)| n == "self")
}
