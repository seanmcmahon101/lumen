//! Small expression evaluator: + - * / ^ ** % ! ( ), functions and constants.

pub fn eval(input: &str) -> Option<f64> {
    let s = input.trim();
    let s = s.strip_prefix('=').unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let mut p = Parser { c: s.chars().collect(), i: 0, work: false };
    let v = p.expr()?;
    p.ws();
    (p.i == p.c.len() && p.work && v.is_finite()).then_some(v)
}

/// Returns (text to show, text to copy).
pub fn format(v: f64) -> (String, String) {
    let a = v.abs();
    if a != 0.0 && !(1e-6..1e15).contains(&a) {
        let s = format!("{v:e}");
        return (s.clone(), s);
    }
    let r = (v * 1e10).round() / 1e10;
    let raw = format!("{r}");
    let (int, frac) = raw.split_once('.').map_or((raw.as_str(), None), |(a, b)| (a, Some(b)));
    let (sign, digits) = int.strip_prefix('-').map_or(("", int), |d| ("-", d));
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let display = match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    };
    (display, raw)
}

struct Parser {
    c: Vec<char>,
    i: usize,
    work: bool, // saw an operator, function or constant (plain "42" is not a calculation)
}

impl Parser {
    fn ws(&mut self) {
        while self.i < self.c.len() && self.c[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.ws();
        self.c.get(self.i).copied()
    }

    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn is_pow(&mut self) -> bool {
        self.peek() == Some('*') && self.c.get(self.i + 1) == Some(&'*')
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v += self.term()?;
            } else if self.eat('-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
            self.work = true;
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.is_pow() {
                return Some(v);
            }
            if self.eat('*') || self.eat('×') {
                v *= self.unary()?;
            } else if self.eat('/') || self.eat('÷') {
                v /= self.unary()?;
            } else {
                return Some(v);
            }
            self.work = true;
        }
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.power()
    }

    fn power(&mut self) -> Option<f64> {
        let v = self.postfix()?;
        let pow = if self.is_pow() {
            self.i += 2;
            true
        } else {
            self.eat('^')
        };
        if pow {
            self.work = true;
            return Some(v.powf(self.unary()?));
        }
        Some(v)
    }

    fn postfix(&mut self) -> Option<f64> {
        let mut v = self.primary()?;
        loop {
            if self.eat('%') {
                v /= 100.0;
            } else if self.eat('!') {
                if !(0.0..=170.0).contains(&v) || v.fract() != 0.0 {
                    return None;
                }
                v = (2..=v as u32).fold(1.0, |acc, k| acc * k as f64);
            } else {
                return Some(v);
            }
            self.work = true;
        }
    }

    fn primary(&mut self) -> Option<f64> {
        let ch = self.peek()?;
        if ch == '(' {
            self.i += 1;
            let v = self.expr()?;
            return self.eat(')').then_some(v);
        }
        if ch.is_ascii_digit() || ch == '.' {
            let start = self.i;
            while self.i < self.c.len() && (self.c[self.i].is_ascii_digit() || self.c[self.i] == '.') {
                self.i += 1;
            }
            // exponent: 1e5, 2.5E-3
            if matches!(self.c.get(self.i), Some('e' | 'E')) {
                let mut j = self.i + 1;
                if matches!(self.c.get(j), Some('+' | '-')) {
                    j += 1;
                }
                if self.c.get(j).is_some_and(|d| d.is_ascii_digit()) {
                    self.i = j;
                    while self.i < self.c.len() && self.c[self.i].is_ascii_digit() {
                        self.i += 1;
                    }
                }
            }
            return self.c[start..self.i].iter().collect::<String>().parse().ok();
        }
        if ch.is_alphabetic() {
            let start = self.i;
            while self.i < self.c.len() && self.c[self.i].is_alphanumeric() {
                self.i += 1;
            }
            let name: String = self.c[start..self.i].iter().collect::<String>().to_lowercase();
            self.work = true;
            match name.as_str() {
                "pi" => return Some(std::f64::consts::PI),
                "e" => return Some(std::f64::consts::E),
                "tau" => return Some(std::f64::consts::TAU),
                _ => {}
            }
            let x = if self.peek() == Some('(') { self.primary()? } else { self.unary()? };
            return Some(match name.as_str() {
                "sqrt" => x.sqrt(),
                "cbrt" => x.cbrt(),
                "sin" => x.sin(),
                "cos" => x.cos(),
                "tan" => x.tan(),
                "asin" => x.asin(),
                "acos" => x.acos(),
                "atan" => x.atan(),
                "ln" => x.ln(),
                "log" => x.log10(),
                "log2" => x.log2(),
                "exp" => x.exp(),
                "abs" => x.abs(),
                "round" => x.round(),
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                _ => return None,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates() {
        assert_eq!(eval("2+3*4"), Some(14.0));
        assert_eq!(eval("(2+3)*4"), Some(20.0));
        assert_eq!(eval("2^10"), Some(1024.0));
        assert_eq!(eval("2**3**2"), Some(512.0));
        assert_eq!(eval("-2^2"), Some(-4.0));
        assert_eq!(eval("sqrt 16 + 1"), Some(5.0));
        assert_eq!(eval("sqrt(16)/2"), Some(2.0));
        assert_eq!(eval("50% * 80"), Some(40.0));
        assert_eq!(eval("5!"), Some(120.0));
        assert_eq!(eval("= 1e3 / 4"), Some(250.0));
        assert!((eval("2*pi").unwrap() - std::f64::consts::TAU).abs() < 1e-12);
    }

    #[test]
    fn rejects_non_math() {
        assert_eq!(eval("42"), None);
        assert_eq!(eval("chrome"), None);
        assert_eq!(eval("2+"), None);
        assert_eq!(eval("(1+2"), None);
        assert_eq!(eval("1/0"), None);
    }

    #[test]
    fn formats() {
        assert_eq!(format(1234567.5), ("1,234,567.5".into(), "1234567.5".into()));
        assert_eq!(format(0.1 + 0.2).1, "0.3");
        assert_eq!(format(-1000.0).0, "-1,000");
    }
}
