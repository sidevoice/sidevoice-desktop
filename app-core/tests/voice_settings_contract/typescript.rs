//! A reader of the little TypeScript a `.d.ts` of settings is made of: `export interface`, `export type`, object types
//! with optional members and an index signature, unions, intersections, string literals and the primitives. Nothing
//! else is understood, and a declaration is read only when it is asked for, so the methods and functions of the rest of
//! the file are never in the way.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Number,
    String,
    Boolean,
    Null,
    Unknown,
    Literal(String),
    Object(Object),
    Union(Vec<Ty>),
    Intersection(Vec<Ty>),
    Reference(String),
}

/// An object type: its members, and whether it also takes any other key (`[key: string]: unknown`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object {
    pub fields: Vec<Field>,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub optional: bool,
    pub ty: Ty,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Text(String),
    Punct(char),
}

fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let (mut found, mut at) = (Vec::new(), 0);
    while at < chars.len() {
        let c = chars[at];
        if c == '/' && chars.get(at + 1) == Some(&'*') {
            at += 2;
            while at < chars.len() && !(chars[at] == '*' && chars.get(at + 1) == Some(&'/')) {
                at += 1;
            }
            at += 2;
        } else if c == '/' && chars.get(at + 1) == Some(&'/') {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '"' {
            let end = (at + 1..chars.len()).find(|&i| chars[i] == '"').expect("a closed string");
            found.push(Token::Text(chars[at + 1..end].iter().collect()));
            at = end + 1;
        } else if c.is_alphanumeric() || c == '_' || c == '$' {
            let end = (at..chars.len()).find(|&i| !(chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$'));
            let end = end.unwrap_or(chars.len());
            found.push(Token::Word(chars[at..end].iter().collect()));
            at = end;
        } else {
            if !c.is_whitespace() {
                found.push(Token::Punct(c));
            }
            at += 1;
        }
    }
    found
}

/// A `.d.ts`, its declarations located and none of them read yet.
pub struct Declarations {
    tokens: Vec<Token>,
    /// Name → where its body starts: after the `{` of an interface, after the `=` of a type.
    at: HashMap<String, (bool, usize)>,
}

impl Declarations {
    pub fn new(source: &str) -> Declarations {
        let tokens = tokens(source);
        let mut at = HashMap::new();
        let word = |i: usize, expected: &str| matches!(tokens.get(i), Some(Token::Word(w)) if w == expected);
        let mut i = 0;
        while i < tokens.len() {
            if word(i, "export") && (word(i + 1, "interface") || word(i + 1, "type")) {
                let interface = word(i + 1, "interface");
                if let Some(Token::Word(name)) = tokens.get(i + 2) {
                    // An interface's body starts at its brace (after any `extends`); a type's after its `=`.
                    let start = (i + 3..tokens.len())
                        .find(|&j| tokens[j] == Token::Punct(if interface { '{' } else { '=' }))
                        .map(|j| j + 1);
                    if let Some(start) = start {
                        at.insert(name.clone(), (interface, start));
                    }
                }
            }
            i += 1;
        }
        Declarations { tokens, at }
    }

    /// The declaration called `name`, as a type (an interface is its object).
    pub fn get(&self, name: &str) -> Result<Ty, String> {
        let &(interface, start) = self.at.get(name).ok_or_else(|| format!("the contract declares no `{name}`"))?;
        let mut reader = Reader { tokens: &self.tokens, at: start };
        let ty = if interface { Ty::Object(reader.members()?) } else { reader.ty()? };
        Ok(ty)
    }

    /// `ty` with references followed, and an intersection of objects made one object.
    pub fn resolve(&self, ty: &Ty) -> Result<Ty, String> {
        match ty {
            Ty::Reference(name) => self.resolve(&self.get(name)?),
            Ty::Intersection(parts) => {
                let mut merged = Object::default();
                for part in parts {
                    match self.resolve(part)? {
                        Ty::Object(object) => {
                            merged.open |= object.open;
                            merged.fields.extend(object.fields);
                        }
                        other => return Err(format!("only objects intersect here, not {other:?}")),
                    }
                }
                Ok(Ty::Object(merged))
            }
            other => Ok(other.clone()),
        }
    }
}

struct Reader<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn punct(&mut self, c: char) -> bool {
        let is = self.peek() == Some(&Token::Punct(c));
        self.at += usize::from(is);
        is
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.punct(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}` at token {}, found {:?}", self.at, self.peek()))
        }
    }

    fn word(&mut self) -> Result<String, String> {
        match self.tokens.get(self.at) {
            Some(Token::Word(w)) => {
                self.at += 1;
                Ok(w.clone())
            }
            other => Err(format!("expected a name at token {}, found {other:?}", self.at)),
        }
    }

    /// The members of an object type, up to the `}` that closes it (which is consumed).
    fn members(&mut self) -> Result<Object, String> {
        let mut object = Object::default();
        while !self.punct('}') {
            if self.punct('[') {
                // `[key: string]: type`
                self.word()?;
                self.expect(':')?;
                self.word()?;
                self.expect(']')?;
                self.expect(':')?;
                self.ty()?;
                object.open = true;
            } else {
                let name = self.word()?;
                let optional = self.punct('?');
                self.expect(':')?;
                let ty = self.ty()?;
                object.fields.push(Field { name, optional, ty });
            }
            if !self.punct(';') {
                self.punct(',');
            }
        }
        Ok(object)
    }

    fn ty(&mut self) -> Result<Ty, String> {
        self.punct('|');
        let mut union = vec![self.intersection()?];
        while self.punct('|') {
            union.push(self.intersection()?);
        }
        Ok(if union.len() == 1 { union.remove(0) } else { Ty::Union(union) })
    }

    fn intersection(&mut self) -> Result<Ty, String> {
        let mut parts = vec![self.primary()?];
        while self.punct('&') {
            parts.push(self.primary()?);
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { Ty::Intersection(parts) })
    }

    fn primary(&mut self) -> Result<Ty, String> {
        if self.punct('{') {
            return self.members().map(Ty::Object);
        }
        if self.punct('(') {
            let ty = self.ty()?;
            self.expect(')')?;
            return Ok(ty);
        }
        if let Some(Token::Text(text)) = self.peek() {
            let text = text.clone();
            self.at += 1;
            return Ok(Ty::Literal(text));
        }
        Ok(match self.word()?.as_str() {
            "number" => Ty::Number,
            "string" => Ty::String,
            "boolean" => Ty::Boolean,
            "null" => Ty::Null,
            "unknown" => Ty::Unknown,
            name => Ty::Reference(name.to_owned()),
        })
    }
}
