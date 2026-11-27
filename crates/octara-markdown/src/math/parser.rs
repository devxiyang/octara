use super::MathError;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    Ordinary,
    Binary,
    Relation,
    Open,
    Close,
    Punctuation,
    Operator,
}

#[derive(Clone, Copy)]
pub(super) enum Font {
    Math,
    Upright,
    Bold,
    Italic,
    Blackboard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Limits {
    Inline,
    Display,
    Always,
}

pub(super) enum Expr {
    Row(Vec<Self>),
    Symbol(char, Class, Limits),
    Space(f32),
    Text(String),
    Font(Font, Box<Self>),
    Fraction(Box<Self>, Box<Self>),
    Radical(Option<Box<Self>>, Box<Self>),
    Scripts { base: Box<Self>, sub: Option<Box<Self>>, sup: Option<Box<Self>>, limits: Limits },
    Delimited(char, Box<Self>, char),
    Grid { rows: Vec<Vec<Self>>, left: char, right: char, align: GridAlign },
    Overline(Box<Self>),
}

#[derive(Clone, Copy)]
pub(super) enum GridAlign {
    Center,
    Left,
    Alternating,
}

impl Expr {
    pub(super) fn class(&self) -> Class {
        match self {
            Self::Symbol(_, class, _) => *class,
            Self::Scripts { base, .. } | Self::Font(_, base) => base.class(),
            _ => Class::Ordinary,
        }
    }
}

pub(super) fn parse(tex: &str) -> Result<Expr, MathError> {
    if tex.len() > 16 * 1024 {
        return Err(MathError::new("formula exceeds 16 KiB"));
    }
    let mut parser = Parser { rest: tex, depth: 0 };
    let (expr, stop) = parser.row()?;
    if stop != Stop::End {
        return Err(MathError::new("unexpected math delimiter"));
    }
    Ok(expr)
}

struct Parser<'a> {
    rest: &'a str,
    depth: u8,
}

#[derive(PartialEq, Eq)]
enum Stop {
    End,
    Brace,
    Bracket,
    Right,
    Column,
    Row,
    Environment,
}

impl<'a> Parser<'a> {
    fn take(&mut self) -> Option<char> {
        let ch = self.rest.chars().next()?;
        self.rest = &self.rest[ch.len_utf8()..];
        Some(ch)
    }

    fn strip(&mut self, prefix: char) -> bool {
        if let Some(rest) = self.rest.strip_prefix(prefix) {
            self.rest = rest;
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        self.rest = self.rest.trim_start();
        while self.strip('%') {
            self.rest = self.rest.find(['\r', '\n']).map_or("", |end| self.rest[end..].trim_start());
        }
    }

    fn control(&mut self) -> Result<&'a str, MathError> {
        let letters = self.rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let end = if letters == 0 { self.rest.chars().next().map_or(0, char::len_utf8) } else { letters };
        if end == 0 {
            return Err(MathError::new("expected a command after backslash"));
        }
        let (command, rest) = self.rest.split_at(end);
        self.rest = rest;
        Ok(command)
    }

    fn command_is(&mut self, name: &str) -> bool {
        let Some(rest) = self.rest.strip_prefix('\\').and_then(|rest| rest.strip_prefix(name)) else {
            return false;
        };
        if rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
            return false;
        }
        self.rest = rest;
        true
    }

    fn nested<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T, MathError>) -> Result<T, MathError> {
        if self.depth >= 48 {
            return Err(MathError::new("formula nesting exceeds 48 levels"));
        }
        self.depth += 1;
        let result = read(self);
        self.depth -= 1;
        result
    }

    fn row(&mut self) -> Result<(Expr, Stop), MathError> {
        self.row_until(false)
    }

    fn row_until(&mut self, bracket: bool) -> Result<(Expr, Stop), MathError> {
        let mut atoms = Vec::new();
        let stop = loop {
            self.skip_space();
            let stop = match self.rest.as_bytes().first() {
                None => Stop::End,
                Some(b'}') => {
                    self.take();
                    Stop::Brace
                }
                Some(b']') if bracket => {
                    self.take();
                    Stop::Bracket
                }
                Some(b'&') => {
                    self.take();
                    Stop::Column
                }
                Some(b'\\') if self.command_is("right") => Stop::Right,
                Some(b'\\') if self.command_is("end") => Stop::Environment,
                Some(b'\\') if self.rest.starts_with("\\\\") => {
                    self.rest = &self.rest[2..];
                    Stop::Row
                }
                _ => {
                    let base = self.atom()?;
                    atoms.push(self.scripts(base)?);
                    continue;
                }
            };
            break stop;
        };
        Ok((Expr::Row(atoms), stop))
    }

    fn atom(&mut self) -> Result<Expr, MathError> {
        self.skip_space();
        self.nested(|parser| match parser.take() {
            Some('{') => parser.group(Stop::Brace),
            Some('\\') => {
                let command = parser.control()?;
                parser.command(command)
            }
            Some('^' | '_' | '}' | '&' | '$') => Err(MathError::new("expected a math atom")),
            Some(ch) => Ok(symbol(ch)),
            None => Err(MathError::new("expected a math argument")),
        })
    }

    fn group(&mut self, stop: Stop) -> Result<Expr, MathError> {
        let (expr, end) = self.row_until(stop == Stop::Bracket)?;
        if end == stop { Ok(expr) } else { Err(MathError::new("unclosed math group")) }
    }

    fn scripts(&mut self, base: Expr) -> Result<Expr, MathError> {
        let mut limits = match base {
            Expr::Symbol(_, _, limits) => limits,
            _ => Limits::Inline,
        };
        self.skip_space();
        if self.command_is("limits") {
            limits = Limits::Always;
        } else if self.command_is("nolimits") {
            limits = Limits::Inline;
        }
        let (mut sub, mut sup) = (None, None);
        loop {
            self.skip_space();
            let slot = match self.rest.as_bytes().first() {
                Some(b'_') => &mut sub,
                Some(b'^') => &mut sup,
                Some(b'\'') => {
                    if sup.is_some() {
                        return Err(MathError::new("duplicate superscript"));
                    }
                    let count = self.rest.bytes().take_while(|&ch| ch == b'\'').count();
                    self.rest = &self.rest[count..];
                    sup = Some(Box::new(Expr::Text("′".repeat(count))));
                    continue;
                }
                _ => break,
            };
            self.take();
            if slot.is_some() {
                return Err(MathError::new("duplicate math script"));
            }
            *slot = Some(Box::new(self.atom()?));
        }
        Ok(if sub.is_none() && sup.is_none() { base } else { Expr::Scripts { base: Box::new(base), sub, sup, limits } })
    }

    fn command(&mut self, command: &str) -> Result<Expr, MathError> {
        Ok(match command {
            "frac" | "dfrac" | "tfrac" => Expr::Fraction(Box::new(self.atom()?), Box::new(self.atom()?)),
            "sqrt" => {
                self.skip_space();
                let degree = if self.strip('[') { Some(Box::new(self.group(Stop::Bracket)?)) } else { None };
                Expr::Radical(degree, Box::new(self.atom()?))
            }
            "text" | "textrm" => Expr::Text(self.text()?),
            "mathrm" | "operatorname" => Expr::Font(Font::Upright, Box::new(self.atom()?)),
            "mathbf" | "boldsymbol" => Expr::Font(Font::Bold, Box::new(self.atom()?)),
            "mathit" => Expr::Font(Font::Italic, Box::new(self.atom()?)),
            "mathbb" => Expr::Font(Font::Blackboard, Box::new(self.atom()?)),
            "overline" | "bar" => Expr::Overline(Box::new(self.atom()?)),
            "left" => {
                let left = self.delimiter()?;
                let (body, stop) = self.row()?;
                if stop != Stop::Right {
                    return Err(MathError::new("expected a matching \\right delimiter"));
                }
                Expr::Delimited(left, Box::new(body), self.delimiter()?)
            }
            "begin" => return self.environment(),
            "," => Expr::Space(1.0 / 6.0),
            ":" | ">" => Expr::Space(2.0 / 9.0),
            ";" => Expr::Space(5.0 / 18.0),
            "!" => Expr::Space(-1.0 / 6.0),
            " " | "enspace" => Expr::Space(0.5),
            "quad" => Expr::Space(1.0),
            "qquad" => Expr::Space(2.0),
            "sin" | "cos" | "tan" | "cot" | "sec" | "csc" | "sinh" | "cosh" | "tanh" | "log" | "ln" | "exp" | "lim" | "min" | "max"
            | "sup" | "inf" | "det" | "gcd" | "arg" | "ker" | "Pr" => Expr::Text(command.to_owned()),
            "sum" => Expr::Symbol('∑', Class::Operator, Limits::Display),
            "prod" => Expr::Symbol('∏', Class::Operator, Limits::Display),
            "coprod" => Expr::Symbol('∐', Class::Operator, Limits::Display),
            "int" => Expr::Symbol('∫', Class::Operator, Limits::Inline),
            "iint" => Expr::Symbol('∬', Class::Operator, Limits::Inline),
            "iiint" => Expr::Symbol('∭', Class::Operator, Limits::Inline),
            "oint" => Expr::Symbol('∮', Class::Operator, Limits::Inline),
            _ => named_symbol(command).map(symbol).ok_or_else(|| MathError::new(format!("unsupported command \\{command}")))?,
        })
    }

    fn delimiter(&mut self) -> Result<char, MathError> {
        self.skip_space();
        let delimiter = match self.take() {
            Some('\\') => named_symbol(self.control()?),
            ch => ch,
        };
        delimiter
            .filter(|&ch| matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '⟨' | '⟩' | '|' | '‖' | '⌊' | '⌋' | '⌈' | '⌉' | '.'))
            .ok_or_else(|| MathError::new("expected a delimiter after \\left or \\right"))
    }

    fn text(&mut self) -> Result<String, MathError> {
        self.skip_space();
        if !self.strip('{') {
            return Err(MathError::new("expected a braced text argument"));
        }
        let mut text = String::new();
        let mut depth = 1;
        while let Some(ch) = self.take() {
            match ch {
                '{' => depth += 1,
                '}' if depth == 1 => return Ok(text),
                '}' => depth -= 1,
                '\\' => match self.control()? {
                    escaped @ ("{" | "}" | "$" | "%" | "&" | "_" | "#" | " ") => text.push_str(escaped),
                    command => return Err(MathError::new(format!("unsupported text command \\{command}"))),
                },
                ch if ch.is_whitespace() => text.push(' '),
                _ => text.push(ch),
            }
        }
        Err(MathError::new("unclosed text group"))
    }

    fn environment(&mut self) -> Result<Expr, MathError> {
        let name = self.text()?;
        let (left, right, align) = match name.as_str() {
            "matrix" => ('.', '.', GridAlign::Center),
            "pmatrix" => ('(', ')', GridAlign::Center),
            "bmatrix" => ('[', ']', GridAlign::Center),
            "Bmatrix" => ('{', '}', GridAlign::Center),
            "vmatrix" => ('|', '|', GridAlign::Center),
            "Vmatrix" => ('‖', '‖', GridAlign::Center),
            "cases" => ('{', '.', GridAlign::Left),
            "aligned" => ('.', '.', GridAlign::Alternating),
            _ => return Err(MathError::new(format!("unsupported environment {name}"))),
        };
        let mut rows = Vec::new();
        let mut cells = Vec::new();
        loop {
            let (cell, stop) = self.row()?;
            let empty = matches!(&cell, Expr::Row(atoms) if atoms.is_empty());
            if !(empty && cells.is_empty() && stop == Stop::Environment && !rows.is_empty()) {
                cells.push(cell);
            }
            match stop {
                Stop::Column => continue,
                Stop::Row => rows.push(std::mem::take(&mut cells)),
                Stop::Environment => {
                    if self.text()? != name {
                        return Err(MathError::new(format!("expected \\end{{{name}}}")));
                    }
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                    return Ok(Expr::Grid { rows, align, left, right });
                }
                _ => return Err(MathError::new(format!("unclosed {name} environment"))),
            }
        }
    }
}

fn symbol(ch: char) -> Expr {
    let ch = if ch == '-' { '−' } else { ch };
    let class = match ch {
        '+' | '−' | '±' | '∓' | '×' | '÷' | '⋅' | '∗' | '⋆' | '∪' | '∩' | '∧' | '∨' | '⊕' | '⊗' | '∖' | '∘' => {
            Class::Binary
        }
        '=' | '≠' | '<' | '>' | '≤' | '≥' | '≈' | '≃' | '≅' | '≡' | '∼' | '∈' | '∉' | '∋' | '⊂' | '⊆' | '⊃' | '⊇' | '∝' | '→' | '←'
        | '↔' | '⇒' | '⇐' | '⇔' | '↦' | '⊥' | '∥' | '≪' | '≫' => Class::Relation,
        '(' | '[' | '{' | '⟨' | '⌊' | '⌈' => Class::Open,
        ')' | ']' | '}' | '⟩' | '⌋' | '⌉' => Class::Close,
        ',' | ';' | ':' => Class::Punctuation,
        '∑' | '∏' | '∐' | '∫' | '∬' | '∭' | '∮' => Class::Operator,
        _ => Class::Ordinary,
    };
    let limits = if matches!(ch, '∑' | '∏' | '∐') { Limits::Display } else { Limits::Inline };
    Expr::Symbol(ch, class, limits)
}

fn named_symbol(name: &str) -> Option<char> {
    Some(match name {
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" => 'ϵ',
        "varepsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" => 'θ',
        "vartheta" => 'ϑ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "varkappa" => 'ϰ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "omicron" => 'ο',
        "pi" => 'π',
        "varpi" => 'ϖ',
        "rho" => 'ρ',
        "varrho" => 'ϱ',
        "sigma" => 'σ',
        "varsigma" => 'ς',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" => 'ϕ',
        "varphi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Theta" => 'Θ',
        "Lambda" => 'Λ',
        "Xi" => 'Ξ',
        "Pi" => 'Π',
        "Sigma" => 'Σ',
        "Upsilon" => 'Υ',
        "Phi" => 'Φ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
        "times" => '×',
        "div" => '÷',
        "cdot" => '⋅',
        "ast" => '∗',
        "star" => '⋆',
        "circ" => '∘',
        "pm" => '±',
        "mp" => '∓',
        "le" | "leq" => '≤',
        "ge" | "geq" => '≥',
        "ne" | "neq" => '≠',
        "approx" => '≈',
        "simeq" => '≃',
        "cong" => '≅',
        "equiv" => '≡',
        "sim" => '∼',
        "propto" => '∝',
        "infty" => '∞',
        "partial" => '∂',
        "nabla" => '∇',
        "ell" => 'ℓ',
        "hbar" => 'ℏ',
        "imath" => 'ı',
        "jmath" => 'ȷ',
        "forall" => '∀',
        "exists" => '∃',
        "neg" | "lnot" => '¬',
        "land" | "wedge" => '∧',
        "lor" | "vee" => '∨',
        "cup" => '∪',
        "cap" => '∩',
        "emptyset" | "varnothing" => '∅',
        "in" => '∈',
        "notin" => '∉',
        "ni" => '∋',
        "subset" => '⊂',
        "subseteq" => '⊆',
        "supset" => '⊃',
        "supseteq" => '⊇',
        "setminus" => '∖',
        "perp" => '⊥',
        "parallel" => '∥',
        "to" | "rightarrow" => '→',
        "gets" | "leftarrow" => '←',
        "leftrightarrow" => '↔',
        "Rightarrow" | "implies" => '⇒',
        "Leftarrow" => '⇐',
        "Leftrightarrow" | "iff" => '⇔',
        "mapsto" => '↦',
        "uparrow" => '↑',
        "downarrow" => '↓',
        "ll" => '≪',
        "gg" => '≫',
        "oplus" => '⊕',
        "otimes" => '⊗',
        "ldots" | "dots" => '…',
        "cdots" => '⋯',
        "vdots" => '⋮',
        "ddots" => '⋱',
        "prime" => '′',
        "langle" => '⟨',
        "rangle" => '⟩',
        "lbrace" | "{" => '{',
        "rbrace" | "}" => '}',
        "lbrack" => '[',
        "rbrack" => ']',
        "lfloor" => '⌊',
        "rfloor" => '⌋',
        "lceil" => '⌈',
        "rceil" => '⌉',
        "vert" | "lvert" | "rvert" => '|',
        "Vert" | "lVert" | "rVert" | "|" => '‖',
        "backslash" => '\\',
        "$" => '$',
        "%" => '%',
        "&" => '&',
        "_" => '_',
        "#" => '#',
        _ => return None,
    })
}
