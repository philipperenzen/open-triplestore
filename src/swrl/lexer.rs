//! Tokenizer shared by the OWL 2 functional-syntax and SWRLAPI rule readers.
//!
//! Quoted strings, `<IRI>`s and prefixed names are single tokens, so a `^`,
//! `,` or `)` inside a literal or an IRI never splits an atom — the failure of
//! the ad-hoc text form, which splits on those characters.

/// One token, with the line it starts on for error messages.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    LParen,
    RParen,
    Comma,
    /// `^`, the SWRLAPI conjunction.
    Caret,
    /// `^^`, before a literal's datatype.
    DoubleCaret,
    /// `->`
    Arrow,
    /// `=`, in functional-syntax `Prefix(ex:=<…>)`.
    Equals,
    /// `<…>`, without the brackets.
    Iri(String),
    /// A prefixed or bare name: `ex:Person`, `:Person`, `Person`, `ex:`.
    Name(String),
    /// `?x`, with the `?`.
    Var(String),
    /// `_:b0`, with the `_:`.
    BlankNode(String),
    /// A quoted string's value, unescaped.
    Str(String),
    /// `@en` after a string, without the `@`.
    Lang(String),
    /// An unquoted number: `17`, `-2.5`, `1e3`.
    Number(String),
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::LParen => f.write_str("'('"),
            Tok::RParen => f.write_str("')'"),
            Tok::Comma => f.write_str("','"),
            Tok::Caret => f.write_str("'^'"),
            Tok::DoubleCaret => f.write_str("'^^'"),
            Tok::Arrow => f.write_str("'->'"),
            Tok::Equals => f.write_str("'='"),
            Tok::Iri(i) => write!(f, "<{i}>"),
            Tok::Name(n) => write!(f, "'{n}'"),
            Tok::Var(v) => write!(f, "'{v}'"),
            Tok::BlankNode(b) => write!(f, "'{b}'"),
            Tok::Str(s) => write!(f, "\"{s}\""),
            Tok::Lang(l) => write!(f, "'@{l}'"),
            Tok::Number(n) => write!(f, "'{n}'"),
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// Split `input` into tokens with their line numbers. `#` starts a comment
/// that runs to the end of the line (outside strings and IRIs).
pub(crate) fn tokenize(input: &str) -> Result<Vec<(Tok, usize)>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < chars.len() {
        let c = chars[i];
        let start_line = line;
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '(' => {
                out.push((Tok::LParen, line));
                i += 1;
            }
            ')' => {
                out.push((Tok::RParen, line));
                i += 1;
            }
            ',' => {
                out.push((Tok::Comma, line));
                i += 1;
            }
            '=' => {
                out.push((Tok::Equals, line));
                i += 1;
            }
            '^' => {
                if chars.get(i + 1) == Some(&'^') {
                    out.push((Tok::DoubleCaret, line));
                    i += 2;
                } else {
                    out.push((Tok::Caret, line));
                    i += 1;
                }
            }
            '-' if chars.get(i + 1) == Some(&'>') => {
                out.push((Tok::Arrow, line));
                i += 2;
            }
            '<' => {
                let mut j = i + 1;
                let mut iri = String::new();
                while j < chars.len() && chars[j] != '>' {
                    if chars[j].is_whitespace() || matches!(chars[j], '<' | '"') {
                        return Err(format!("line {line}: unterminated or malformed <IRI>"));
                    }
                    iri.push(chars[j]);
                    j += 1;
                }
                if j >= chars.len() {
                    return Err(format!("line {line}: unterminated <IRI>"));
                }
                out.push((Tok::Iri(iri), line));
                i = j + 1;
            }
            '"' => {
                let mut j = i + 1;
                let mut value = String::new();
                loop {
                    let Some(&ch) = chars.get(j) else {
                        return Err(format!("line {start_line}: unterminated string"));
                    };
                    match ch {
                        '"' => break,
                        '\\' => {
                            let esc = chars
                                .get(j + 1)
                                .ok_or_else(|| format!("line {line}: unterminated string"))?;
                            value.push(match esc {
                                '"' => '"',
                                '\\' => '\\',
                                'n' => '\n',
                                't' => '\t',
                                'r' => '\r',
                                other => {
                                    return Err(format!("line {line}: unknown escape \\{other}"))
                                }
                            });
                            j += 2;
                        }
                        '\n' => {
                            line += 1;
                            value.push('\n');
                            j += 1;
                        }
                        other => {
                            value.push(other);
                            j += 1;
                        }
                    }
                }
                out.push((Tok::Str(value), start_line));
                i = j + 1;
                if chars.get(i) == Some(&'@') {
                    let mut k = i + 1;
                    let mut lang = String::new();
                    while k < chars.len() && (chars[k].is_ascii_alphanumeric() || chars[k] == '-') {
                        lang.push(chars[k]);
                        k += 1;
                    }
                    if lang.is_empty() {
                        return Err(format!("line {line}: '@' without a language tag"));
                    }
                    out.push((Tok::Lang(lang), line));
                    i = k;
                }
            }
            '?' => {
                let mut j = i + 1;
                let mut name = String::from("?");
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    name.push(chars[j]);
                    j += 1;
                }
                if name.len() == 1 {
                    return Err(format!("line {line}: '?' without a variable name"));
                }
                out.push((Tok::Var(name), line));
                i = j;
            }
            '_' if chars.get(i + 1) == Some(&':') => {
                let mut j = i + 2;
                let mut name = String::from("_:");
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    name.push(chars[j]);
                    j += 1;
                }
                out.push((Tok::BlankNode(name), line));
                i = j;
            }
            c if c.is_ascii_digit()
                || (matches!(c, '+' | '-')
                    && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].is_ascii_digit()
                        || matches!(chars[j], '.' | 'e' | 'E')
                        || (matches!(chars[j], '+' | '-') && matches!(chars[j - 1], 'e' | 'E')))
                {
                    j += 1;
                }
                let mut number: String = chars[i..j].iter().collect();
                // A trailing dot ends a statement, not the number.
                while number.ends_with('.') {
                    number.pop();
                    j -= 1;
                }
                out.push((Tok::Number(number), line));
                i = j;
            }
            c if c.is_alphabetic() || c == '_' || c == ':' => {
                let mut j = i;
                while j < chars.len() && is_name_char(chars[j]) {
                    j += 1;
                }
                let mut name: String = chars[i..j].iter().collect();
                while name.ends_with('.') {
                    name.pop();
                    j -= 1;
                }
                out.push((Tok::Name(name), line));
                i = j;
            }
            other => return Err(format!("line {line}: unexpected character '{other}'")),
        }
    }
    Ok(out)
}

/// A cursor over tokens.
pub(crate) struct Tokens {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

impl Tokens {
    pub(crate) fn new(input: &str) -> Result<Self, String> {
        Ok(Tokens {
            toks: tokenize(input)?,
            pos: 0,
        })
    }

    pub(crate) fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    pub(crate) fn peek_at(&self, offset: usize) -> Option<&Tok> {
        self.toks.get(self.pos + offset).map(|(t, _)| t)
    }

    pub(crate) fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|(t, _)| t.clone());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// The line of the next token (or of the last one at the end).
    pub(crate) fn line(&self) -> usize {
        self.toks
            .get(self.pos)
            .or_else(|| self.toks.last())
            .map(|(_, l)| *l)
            .unwrap_or(1)
    }

    /// An error message located at the next token.
    pub(crate) fn error(&self, msg: impl std::fmt::Display) -> String {
        format!("line {}: {msg}", self.line())
    }

    /// Consume `want` or fail naming what was found.
    pub(crate) fn expect(&mut self, want: &Tok) -> Result<(), String> {
        match self.peek() {
            Some(t) if t == want => {
                self.pos += 1;
                Ok(())
            }
            Some(t) => Err(self.error(format!("expected {want}, found {t}"))),
            None => Err(self.error(format!("expected {want}, found the end of the input"))),
        }
    }

    /// Skip a balanced `( … )` group; the next token must be `(`.
    pub(crate) fn skip_group(&mut self) -> Result<(), String> {
        self.expect(&Tok::LParen)?;
        let mut depth = 1;
        while depth > 0 {
            match self.next() {
                Some(Tok::LParen) => depth += 1,
                Some(Tok::RParen) => depth -= 1,
                Some(_) => {}
                None => return Err(self.error("unbalanced parentheses")),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_iris_are_single_tokens() {
        let toks: Vec<Tok> = tokenize(
            r#"ex:p(?x, "a ^ b, c)"@en) ^ <http://ex/q#r>(?x, "1"^^xsd:int) -> :T(?x) # done"#,
        )
        .unwrap()
        .into_iter()
        .map(|(t, _)| t)
        .collect();
        assert_eq!(
            toks,
            vec![
                Tok::Name("ex:p".into()),
                Tok::LParen,
                Tok::Var("?x".into()),
                Tok::Comma,
                Tok::Str("a ^ b, c)".into()),
                Tok::Lang("en".into()),
                Tok::RParen,
                Tok::Caret,
                Tok::Iri("http://ex/q#r".into()),
                Tok::LParen,
                Tok::Var("?x".into()),
                Tok::Comma,
                Tok::Str("1".into()),
                Tok::DoubleCaret,
                Tok::Name("xsd:int".into()),
                Tok::RParen,
                Tok::Arrow,
                Tok::Name(":T".into()),
                Tok::LParen,
                Tok::Var("?x".into()),
                Tok::RParen,
            ]
        );
        assert_eq!(
            tokenize("-2.5 17 1e3").unwrap().len(),
            3,
            "numbers, signed and with exponents"
        );
        assert!(tokenize("\"open").is_err());
        assert!(tokenize("<http://ex/ a>").is_err());
    }
}
