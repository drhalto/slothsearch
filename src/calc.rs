//! Inline calculator and unit converter.
//!
//! Arithmetic with + - * / ^ (or **), `mod`, factorial `!`, implicit multiplication
//! (`2pi`, `3(4+5)`), functions (sqrt, sin, log...), constants (pi, e, tau, phi), and
//! percentages the way people say them: `15% of 80`, `80 + 15%`, `80 - 15%`.
//! Conversions: `10 km to mi`, `72 f in c`, `5 gb to mb`, `3 cups as ml`.

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// What the row shows, with thousands separators: "1,234.5".
    pub display: String,
    /// What Enter copies: "1234.5".
    pub copy: String,
    /// A short echo of what was computed: "10 km → mi".
    pub detail: String,
}

pub fn eval(input: &str) -> Option<Answer> {
    let s = input.trim();
    if s.is_empty() || s.len() > 200 || looks_like_date(s) {
        return None;
    }
    if let Some(a) = convert(s) {
        return Some(a);
    }
    let tokens = tokenize(s)?;
    if !looks_like_math(&tokens) {
        return None;
    }
    let mut p = Parser { t: &tokens, i: 0 };
    let v = p.expr()?;
    if p.i != tokens.len() || !v.v.is_finite() {
        return None;
    }
    let v = v.resolve();
    Some(Answer { display: format_num(v, true), copy: format_num(v, false), detail: s.to_string() })
}

fn looks_like_date(s: &str) -> bool {
    // 2024-10-11 and 10/11/2024 are dates, not subtraction.
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| r.clone().all(|i| b.get(i).is_some_and(u8::is_ascii_digit));
    (b.len() == 10 && digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10))
        || (s.matches('/').count() == 2 && s.chars().all(|c| c.is_ascii_digit() || c == '/'))
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    // "1,000,000" is a number unless commas could be argument separators.
    let owned;
    let s = if !s.contains('(') && s.contains(',') {
        owned = s.replace(',', "");
        owned.as_str()
    } else {
        s
    };
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            ' ' | '\t' | '_' => i += 1,
            '0'..='9' | '.' => {
                if ch == '0' && matches!(c.get(i + 1), Some('x' | 'X' | 'b' | 'B')) {
                    let radix = if matches!(c[i + 1], 'x' | 'X') { 16 } else { 2 };
                    let start = i + 2;
                    let mut j = start;
                    while j < c.len() && c[j].is_digit(radix) {
                        j += 1;
                    }
                    if j == start {
                        return None;
                    }
                    let text: String = c[start..j].iter().collect();
                    out.push(Tok::Num(i64::from_str_radix(&text, radix).ok()? as f64));
                    i = j;
                    continue;
                }
                let start = i;
                while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                    i += 1;
                }
                // Exponent only when digits follow, so "2e" stays 2·e.
                if i < c.len() && matches!(c[i], 'e' | 'E') {
                    let mut j = i + 1;
                    if j < c.len() && matches!(c[j], '+' | '-') {
                        j += 1;
                    }
                    if j < c.len() && c[j].is_ascii_digit() {
                        while j < c.len() && c[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    }
                }
                let text: String = c[start..i].iter().collect();
                out.push(Tok::Num(text.parse().ok()?));
            }
            'a'..='z' | 'A'..='Z' | 'π' | 'φ' | 'τ' => {
                let start = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == 'π') {
                    i += 1;
                }
                out.push(Tok::Ident(c[start..i].iter().collect::<String>().to_lowercase()));
            }
            '*' if c.get(i + 1) == Some(&'*') => {
                out.push(Tok::Op('^'));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '^' | '%' | '!' => {
                out.push(Tok::Op(ch));
                i += 1;
            }
            '×' | '·' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '−' => {
                out.push(Tok::Op('-'));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

const FUNCTIONS: &[&str] = &[
    "sqrt", "cbrt", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "ln", "log", "log2",
    "log10", "exp", "floor", "ceil", "round", "trunc", "min", "max", "pow", "hypot", "deg", "rad",
];

fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "pi" | "π" => std::f64::consts::PI,
        "e" => std::f64::consts::E,
        "tau" | "τ" => std::f64::consts::TAU,
        "phi" | "φ" => 1.618_033_988_749_895,
        _ => return None,
    })
}

/// A lone number or word isn't a calculation; it needs an operator or a function.
fn looks_like_math(t: &[Tok]) -> bool {
    let has_op = t.iter().enumerate().any(|(i, tok)| match tok {
        Tok::Op('-') | Tok::Op('+') => i > 0,
        Tok::Op(_) => true,
        _ => false,
    });
    let has_fn = t.iter().any(|tok| matches!(tok, Tok::Ident(n) if FUNCTIONS.contains(&n.as_str())));
    let all_known = t.iter().all(|tok| match tok {
        Tok::Ident(n) => FUNCTIONS.contains(&n.as_str()) || constant(n).is_some() || n == "mod" || n == "of",
        _ => true,
    });
    let implicit = t.windows(2).any(|w| matches!((&w[0], &w[1]), (Tok::Num(_), Tok::Ident(_) | Tok::LParen)));
    all_known && (has_op || has_fn || implicit)
}

#[derive(Clone, Copy, Debug)]
struct Val {
    v: f64,
    /// A bare percentage (`15%`), still to be applied relative to something.
    pct: bool,
}

impl Val {
    fn resolve(self) -> f64 {
        if self.pct { self.v / 100.0 } else { self.v }
    }
}

fn val(v: f64) -> Val {
    Val { v, pct: false }
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == Some(tok) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<Val> {
        let mut left = self.term()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(c @ ('+' | '-'))) => *c,
                _ => break,
            };
            self.i += 1;
            let right = self.term()?;
            let l = left.resolve();
            left = match (op, right.pct) {
                ('+', true) => val(l * (1.0 + right.v / 100.0)),
                ('-', true) => val(l * (1.0 - right.v / 100.0)),
                ('+', false) => val(l + right.v),
                _ => val(l - right.v),
            };
        }
        Some(left)
    }

    fn term(&mut self) -> Option<Val> {
        let mut left = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Op('*')) => {
                    self.i += 1;
                    let r = self.unary()?;
                    left = val(left.resolve() * r.resolve());
                }
                Some(Tok::Op('/')) => {
                    self.i += 1;
                    let r = self.unary()?.resolve();
                    if r == 0.0 {
                        return None;
                    }
                    left = val(left.resolve() / r);
                }
                Some(Tok::Op('%')) => {
                    // Postfix % was handled in `postfix`; here it's modulo: `10 % 3`.
                    self.i += 1;
                    let r = self.unary()?.resolve();
                    if r == 0.0 {
                        return None;
                    }
                    left = val(left.resolve().rem_euclid(r));
                }
                Some(Tok::Ident(n)) if n == "mod" => {
                    self.i += 1;
                    let r = self.unary()?.resolve();
                    if r == 0.0 {
                        return None;
                    }
                    left = val(left.resolve().rem_euclid(r));
                }
                Some(Tok::Ident(n)) if n == "of" && left.pct => {
                    self.i += 1;
                    let r = self.unary()?.resolve();
                    left = val(left.resolve() * r);
                }
                Some(Tok::Num(_) | Tok::LParen) | Some(Tok::Ident(_)) => {
                    // Implicit multiplication: 2pi, 3(4), (1)(2).
                    let r = self.unary()?;
                    left = val(left.resolve() * r.resolve());
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<Val> {
        if self.eat(&Tok::Op('-')) {
            let v = self.unary()?;
            return Some(Val { v: -v.v, pct: v.pct });
        }
        if self.eat(&Tok::Op('+')) {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Option<Val> {
        let base = self.postfix()?;
        if self.eat(&Tok::Op('^')) {
            let exp = self.unary()?;
            return Some(val(base.resolve().powf(exp.resolve())));
        }
        Some(base)
    }

    fn postfix(&mut self) -> Option<Val> {
        let mut v = self.primary()?;
        loop {
            match self.peek() {
                Some(Tok::Op('!')) => {
                    self.i += 1;
                    v = val(factorial(v.resolve())?);
                }
                Some(Tok::Op('%')) => {
                    // Percent sign unless an operand follows (then it's modulo).
                    let next = self.t.get(self.i + 1);
                    let operand_follows = matches!(next, Some(Tok::Num(_) | Tok::LParen))
                        || matches!(next, Some(Tok::Ident(n)) if n != "of" && n != "mod");
                    if operand_follows {
                        break;
                    }
                    self.i += 1;
                    v = Val { v: v.resolve(), pct: true };
                }
                _ => break,
            }
        }
        Some(v)
    }

    fn primary(&mut self) -> Option<Val> {
        match self.peek()?.clone() {
            Tok::Num(n) => {
                self.i += 1;
                Some(val(n))
            }
            Tok::LParen => {
                self.i += 1;
                let v = self.expr()?;
                self.eat(&Tok::RParen); // tolerate a missing close paren while typing
                Some(v)
            }
            Tok::Ident(name) => {
                self.i += 1;
                if let Some(c) = constant(&name) {
                    return Some(val(c));
                }
                if !FUNCTIONS.contains(&name.as_str()) {
                    return None;
                }
                let args = if self.eat(&Tok::LParen) {
                    let mut args = vec![self.expr()?.resolve()];
                    while self.eat(&Tok::Comma) {
                        args.push(self.expr()?.resolve());
                    }
                    self.eat(&Tok::RParen);
                    args
                } else {
                    // sqrt 16, sin pi
                    vec![self.power()?.resolve()]
                };
                apply(&name, &args).map(val)
            }
            _ => None,
        }
    }
}

fn factorial(n: f64) -> Option<f64> {
    if n < 0.0 || n.fract() != 0.0 || n > 170.0 {
        return None;
    }
    Some((1..=n as u64).fold(1.0, |acc, k| acc * k as f64))
}

fn apply(name: &str, a: &[f64]) -> Option<f64> {
    let x = *a.first()?;
    let v = match (name, a.len()) {
        ("sqrt", 1) => x.sqrt(),
        ("cbrt", 1) => x.cbrt(),
        ("abs", 1) => x.abs(),
        ("sin", 1) => x.sin(),
        ("cos", 1) => x.cos(),
        ("tan", 1) => x.tan(),
        ("asin", 1) => x.asin(),
        ("acos", 1) => x.acos(),
        ("atan", 1) => x.atan(),
        ("sinh", 1) => x.sinh(),
        ("cosh", 1) => x.cosh(),
        ("tanh", 1) => x.tanh(),
        ("ln", 1) => x.ln(),
        ("log" | "log10", 1) => x.log10(),
        ("log", 2) => x.ln() / a[1].ln(),
        ("log2", 1) => x.log2(),
        ("exp", 1) => x.exp(),
        ("floor", 1) => x.floor(),
        ("ceil", 1) => x.ceil(),
        ("round", 1) => x.round(),
        ("trunc", 1) => x.trunc(),
        ("deg", 1) => x.to_degrees(),
        ("rad", 1) => x.to_radians(),
        ("min", _) => a.iter().copied().fold(f64::INFINITY, f64::min),
        ("max", _) => a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ("pow", 2) => x.powf(a[1]),
        ("hypot", 2) => x.hypot(a[1]),
        _ => return None,
    };
    v.is_finite().then_some(v)
}

/// Up to 10 significant digits, no trailing zeros; scientific for very large/small.
pub fn format_num(v: f64, separators: bool) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-6..1e15).contains(&a) {
        return format!("{v:.6e}").replace("e", "e+").replace("e+-", "e-").replace(".000000", "");
    }
    let decimals = (9 - a.log10().floor() as i32).clamp(0, 10) as usize;
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" {
        s = "0".into();
    }
    if !separators {
        return s;
    }
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') { ("-", r) } else { ("", s.as_str()) };
    let (int, frac) = rest.split_once('.').map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

// ---------------------------------------------------------------------------------------
// Units

#[derive(Clone, Copy, PartialEq, Debug)]
enum Dim {
    Length,
    Mass,
    Volume,
    Time,
    Data,
    Speed,
    Area,
    Temp,
}

struct Unit {
    names: &'static [&'static str],
    /// Short label for output.
    label: &'static str,
    dim: Dim,
    /// Size of one unit in the dimension's base unit (m, kg, L, s, byte, m/s, m²).
    factor: f64,
}

const UNITS: &[Unit] = &[
    Unit { names: &["mm", "millimeter", "millimeters", "millimetre", "millimetres"], label: "mm", dim: Dim::Length, factor: 0.001 },
    Unit { names: &["cm", "centimeter", "centimeters", "centimetre", "centimetres"], label: "cm", dim: Dim::Length, factor: 0.01 },
    Unit { names: &["m", "meter", "meters", "metre", "metres"], label: "m", dim: Dim::Length, factor: 1.0 },
    Unit { names: &["km", "kilometer", "kilometers", "kilometre", "kilometres"], label: "km", dim: Dim::Length, factor: 1000.0 },
    Unit { names: &["in", "inch", "inches", "\""], label: "in", dim: Dim::Length, factor: 0.0254 },
    Unit { names: &["ft", "foot", "feet", "'"], label: "ft", dim: Dim::Length, factor: 0.3048 },
    Unit { names: &["yd", "yard", "yards"], label: "yd", dim: Dim::Length, factor: 0.9144 },
    Unit { names: &["mi", "mile", "miles"], label: "mi", dim: Dim::Length, factor: 1609.344 },
    Unit { names: &["nmi", "nautical mile", "nautical miles"], label: "nmi", dim: Dim::Length, factor: 1852.0 },
    Unit { names: &["mg", "milligram", "milligrams"], label: "mg", dim: Dim::Mass, factor: 1e-6 },
    Unit { names: &["g", "gram", "grams"], label: "g", dim: Dim::Mass, factor: 0.001 },
    Unit { names: &["kg", "kilo", "kilos", "kilogram", "kilograms"], label: "kg", dim: Dim::Mass, factor: 1.0 },
    Unit { names: &["oz", "ounce", "ounces"], label: "oz", dim: Dim::Mass, factor: 0.028_349_523_125 },
    Unit { names: &["lb", "lbs", "pound", "pounds"], label: "lb", dim: Dim::Mass, factor: 0.453_592_37 },
    Unit { names: &["st", "stone", "stones"], label: "st", dim: Dim::Mass, factor: 6.350_293_18 },
    Unit { names: &["t", "tonne", "tonnes", "metric ton", "metric tons"], label: "t", dim: Dim::Mass, factor: 1000.0 },
    Unit { names: &["ton", "tons", "short ton", "short tons"], label: "ton", dim: Dim::Mass, factor: 907.184_74 },
    Unit { names: &["ml", "milliliter", "milliliters", "millilitre", "millilitres"], label: "mL", dim: Dim::Volume, factor: 0.001 },
    Unit { names: &["l", "liter", "liters", "litre", "litres"], label: "L", dim: Dim::Volume, factor: 1.0 },
    Unit { names: &["tsp", "teaspoon", "teaspoons"], label: "tsp", dim: Dim::Volume, factor: 0.004_928_921_593_75 },
    Unit { names: &["tbsp", "tablespoon", "tablespoons"], label: "tbsp", dim: Dim::Volume, factor: 0.014_786_764_781_25 },
    Unit { names: &["floz", "fl oz", "fluid ounce", "fluid ounces"], label: "fl oz", dim: Dim::Volume, factor: 0.029_573_529_562_5 },
    Unit { names: &["cup", "cups"], label: "cups", dim: Dim::Volume, factor: 0.236_588_236_5 },
    Unit { names: &["pt", "pint", "pints"], label: "pt", dim: Dim::Volume, factor: 0.473_176_473 },
    Unit { names: &["qt", "quart", "quarts"], label: "qt", dim: Dim::Volume, factor: 0.946_352_946 },
    Unit { names: &["gal", "gallon", "gallons"], label: "gal", dim: Dim::Volume, factor: 3.785_411_784 },
    Unit { names: &["ms", "millisecond", "milliseconds"], label: "ms", dim: Dim::Time, factor: 0.001 },
    Unit { names: &["s", "sec", "secs", "second", "seconds"], label: "s", dim: Dim::Time, factor: 1.0 },
    Unit { names: &["min", "mins", "minute", "minutes"], label: "min", dim: Dim::Time, factor: 60.0 },
    Unit { names: &["h", "hr", "hrs", "hour", "hours"], label: "h", dim: Dim::Time, factor: 3600.0 },
    Unit { names: &["d", "day", "days"], label: "days", dim: Dim::Time, factor: 86_400.0 },
    Unit { names: &["wk", "week", "weeks"], label: "weeks", dim: Dim::Time, factor: 604_800.0 },
    Unit { names: &["month", "months"], label: "months", dim: Dim::Time, factor: 2_629_800.0 },
    Unit { names: &["yr", "year", "years"], label: "years", dim: Dim::Time, factor: 31_557_600.0 },
    Unit { names: &["bit", "bits"], label: "bits", dim: Dim::Data, factor: 0.125 },
    Unit { names: &["b", "byte", "bytes"], label: "bytes", dim: Dim::Data, factor: 1.0 },
    Unit { names: &["kb", "kib", "kilobyte", "kilobytes"], label: "KB", dim: Dim::Data, factor: 1024.0 },
    Unit { names: &["mb", "mib", "megabyte", "megabytes"], label: "MB", dim: Dim::Data, factor: 1_048_576.0 },
    Unit { names: &["gb", "gib", "gigabyte", "gigabytes"], label: "GB", dim: Dim::Data, factor: 1_073_741_824.0 },
    Unit { names: &["tb", "tib", "terabyte", "terabytes"], label: "TB", dim: Dim::Data, factor: 1_099_511_627_776.0 },
    Unit { names: &["m/s", "mps"], label: "m/s", dim: Dim::Speed, factor: 1.0 },
    Unit { names: &["km/h", "kph", "kmh", "kmph"], label: "km/h", dim: Dim::Speed, factor: 1.0 / 3.6 },
    Unit { names: &["mph"], label: "mph", dim: Dim::Speed, factor: 0.447_04 },
    Unit { names: &["ft/s", "fps"], label: "ft/s", dim: Dim::Speed, factor: 0.3048 },
    Unit { names: &["kn", "knot", "knots", "kt"], label: "kn", dim: Dim::Speed, factor: 0.514_444 },
    Unit { names: &["sqm", "m2", "m²", "sq m", "square meters", "square metres"], label: "m²", dim: Dim::Area, factor: 1.0 },
    Unit { names: &["sqft", "ft2", "ft²", "sq ft", "square feet"], label: "sq ft", dim: Dim::Area, factor: 0.092_903_04 },
    Unit { names: &["km2", "km²", "sq km"], label: "km²", dim: Dim::Area, factor: 1e6 },
    Unit { names: &["sqmi", "mi2", "mi²", "sq mi", "square miles"], label: "sq mi", dim: Dim::Area, factor: 2_589_988.110_336 },
    Unit { names: &["acre", "acres", "ac"], label: "acres", dim: Dim::Area, factor: 4046.856_422_4 },
    Unit { names: &["ha", "hectare", "hectares"], label: "ha", dim: Dim::Area, factor: 10_000.0 },
    Unit { names: &["c", "°c", "celsius", "degc"], label: "°C", dim: Dim::Temp, factor: 0.0 },
    Unit { names: &["f", "°f", "fahrenheit", "degf"], label: "°F", dim: Dim::Temp, factor: 1.0 },
    Unit { names: &["k", "kelvin"], label: "K", dim: Dim::Temp, factor: 2.0 },
];

fn unit(name: &str) -> Option<&'static Unit> {
    let n = name.trim().to_lowercase();
    UNITS.iter().find(|u| u.names.contains(&n.as_str()))
}

fn to_kelvin(v: f64, u: &Unit) -> f64 {
    match u.factor as i32 {
        0 => v + 273.15,
        1 => (v - 32.0) * 5.0 / 9.0 + 273.15,
        _ => v,
    }
}

fn from_kelvin(k: f64, u: &Unit) -> f64 {
    match u.factor as i32 {
        0 => k - 273.15,
        1 => (k - 273.15) * 9.0 / 5.0 + 32.0,
        _ => k,
    }
}

/// `<amount> <unit> to|in|as|into <unit>`, where amount may be an expression.
fn convert(s: &str) -> Option<Answer> {
    let lower = s.to_lowercase();
    let (left, right) = [" to ", " into ", " as ", " in "].iter().find_map(|kw| lower.rsplit_once(kw))?;
    let to = unit(right)?;
    // Split the left side into an amount and a unit: longest unit suffix wins.
    let left = left.trim();
    let (amount_text, from) = (1..=left.len().min(20))
        .rev()
        .filter(|&n| left.is_char_boundary(left.len() - n))
        .find_map(|n| {
            let (a, u) = left.split_at(left.len() - n);
            let amount = a.trim_end();
            // The unit must be its own word ("5km" or "5 km"), and an amount must remain.
            let joined = !a.ends_with(' ') && a.chars().last().is_some_and(|c| c.is_alphabetic());
            (!amount.is_empty() && !joined).then(|| unit(u).map(|u| (amount, u))).flatten()
        })?;
    if from.dim != to.dim {
        return None;
    }
    let amount = match amount_text.parse::<f64>() {
        Ok(v) => v,
        Err(_) => {
            let tokens = tokenize(amount_text)?;
            let mut p = Parser { t: &tokens, i: 0 };
            let v = p.expr()?;
            if p.i != tokens.len() {
                return None;
            }
            v.resolve()
        }
    };
    let out = if from.dim == Dim::Temp {
        from_kelvin(to_kelvin(amount, from), to)
    } else {
        amount * from.factor / to.factor
    };
    if !out.is_finite() {
        return None;
    }
    Some(Answer {
        display: format!("{} {}", format_num(out, true), to.label),
        copy: format_num(out, false),
        detail: format!("{} {} → {}", format_num(amount, true), from.label, to.label),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<String> {
        eval(s).map(|a| a.copy)
    }

    #[test]
    fn arithmetic() {
        assert_eq!(v("2+2").as_deref(), Some("4"));
        assert_eq!(v("2 + 3 * 4").as_deref(), Some("14"));
        assert_eq!(v("(2+3)*4").as_deref(), Some("20"));
        assert_eq!(v("2^10").as_deref(), Some("1024"));
        assert_eq!(v("2**3").as_deref(), Some("8"));
        assert_eq!(v("-2^2").as_deref(), Some("-4"));
        assert_eq!(v("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(v("10 % 3").as_deref(), Some("1"));
        assert_eq!(v("10 mod 4").as_deref(), Some("2"));
        assert_eq!(v("5!").as_deref(), Some("120"));
        assert_eq!(v("2pi").as_deref(), Some("6.283185307"));
        assert_eq!(v("sqrt(16)+1").as_deref(), Some("5"));
        assert_eq!(v("sqrt 16").as_deref(), Some("4"));
        assert_eq!(v("max(1, 7, 3)").as_deref(), Some("7"));
        assert_eq!(v("0x1F + 1").as_deref(), Some("32"));
        assert_eq!(v("1,000,000 / 4").as_deref(), Some("250000"));
        assert_eq!(v("1.5e3 * 2").as_deref(), Some("3000"));
        assert_eq!(v("(1+2").as_deref(), Some("3"));
    }

    #[test]
    fn percentages() {
        assert_eq!(v("15% of 80").as_deref(), Some("12"));
        assert_eq!(v("80 + 15%").as_deref(), Some("92"));
        assert_eq!(v("80 - 25%").as_deref(), Some("60"));
        assert_eq!(v("200 * 10%").as_deref(), Some("20"));
        assert_eq!(v("50%").as_deref(), Some("0.5"));
    }

    #[test]
    fn percentages_in_numeric_contexts() {
        assert_eq!(v("(50%)^2").as_deref(), Some("0.25"));
        assert_eq!(v("4^(50%)").as_deref(), Some("2"));
        assert_eq!(v("sqrt(50%)").as_deref(), Some("0.7071067812"));
        assert_eq!(v("sqrt 25%").as_deref(), Some("0.5"));
        assert_eq!(v("pow(25%, 50%)").as_deref(), Some("0.5"));
        assert_eq!(v("min(75%, 25%)").as_deref(), Some("0.25"));
        assert_eq!(v("(100%)!").as_deref(), Some("1"));
        assert_eq!(v("50% m to cm").as_deref(), Some("50"));
        assert_eq!(v("(50% + 15%) m to cm").as_deref(), Some("57.5"));
    }

    #[test]
    fn not_math() {
        assert_eq!(v("42"), None);
        assert_eq!(v("chrome"), None);
        assert_eq!(v("e"), None);
        assert_eq!(v("c++"), None);
        assert_eq!(v("2024-10-11"), None);
        assert_eq!(v("10/11/2026"), None);
        assert_eq!(v("1/0"), None);
        assert_eq!(v("x-1"), None);
        assert_eq!(v("node-sass"), None);
    }

    #[test]
    fn formatting() {
        assert_eq!(eval("1000*1234.5").unwrap().display, "1,234,500");
        assert_eq!(eval("-1234567/10").unwrap().display, "-123,456.7");
        assert_eq!(format_num(1e20, false), "1e+20");
    }

    #[test]
    fn conversions() {
        assert_eq!(eval("10 km to mi").unwrap().display, "6.213711922 mi");
        assert_eq!(eval("72 f in c").unwrap().copy, "22.22222222");
        assert_eq!(eval("100 c to f").unwrap().copy, "212");
        assert_eq!(eval("5 gb to mb").unwrap().copy, "5120");
        assert_eq!(eval("3 cups as ml").unwrap().copy, "709.7647095");
        assert_eq!(eval("2 hours in minutes").unwrap().copy, "120");
        assert_eq!(eval("6 ft to cm").unwrap().copy, "182.88");
        assert_eq!(eval("5km to m").unwrap().copy, "5000");
        assert_eq!(eval("60 mph to km/h").unwrap().copy, "96.56064");
        assert_eq!(eval("10 in to cm").unwrap().copy, "25.4");
        assert_eq!(eval("2*3 kg to lb").unwrap().copy, "13.22773573");
        assert_eq!(eval("5 kg to km"), None);
        assert_eq!(eval("search in files"), None);
    }
}
