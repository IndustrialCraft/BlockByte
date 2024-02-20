use immutable_string::ImmutableString;
use std::fmt::{Debug, Formatter};
use std::str::FromStr;

pub struct TokenReader {
    tokens: Vec<Token>,
}
impl TokenReader {
    pub fn lex(text: &str) -> TokenReader {
        let text: Vec<char> = text.chars().collect();
        let mut tokens = Vec::new();

        let mut i = 0;
        loop {
            if i >= text.len() {
                break;
            }
            let char = CharacterType::from_char(text[i]);
            match char {
                CharacterType::Alpha(char) => {
                    let mut string = String::new();
                    string.push(char);
                    while let Some(alpha) = text
                        .get(i + 1)
                        .cloned()
                        .and_then(|char| CharacterType::from_char(char).as_alphanum())
                    {
                        string.push(alpha);
                        i += 1;
                    }
                    tokens.push(match string.as_str() {
                        "fn" => Token::Fn,
                        "for" => Token::For,
                        "if" => Token::If,
                        "in" => Token::In,
                        identifier => Token::Identifier(identifier.into()),
                    });
                }
                CharacterType::Number(digit) => {
                    let mut numbers = String::new();
                    numbers.push((digit + '0' as u8) as char);
                    let mut got_dot = false;
                    while text[i + 1].is_numeric()
                        || (text[i + 1] == '.' && text[i + 2] != '.' && !got_dot)
                    {
                        if text[i + 1] == '.' {
                            got_dot = true;
                        }
                        numbers.push(text[i + 1]);
                        i += 1;
                    }
                    if got_dot {
                        tokens.push(Token::Float(f64::from_str(numbers.as_str()).unwrap()))
                    } else {
                        tokens.push(Token::Int(i64::from_str(numbers.as_str()).unwrap()))
                    }
                }
                CharacterType::Quote => {
                    let mut string = String::new();
                    while text[i + 1] != '"' {
                        string.push(text[i + 1]);
                        i += 1;
                    }
                    i += 1;
                    tokens.push(Token::String(string.into()));
                }
                CharacterType::Dot => {
                    if text[i + 1] == '.' {
                        if text[i + 2] == '=' {
                            tokens.push(Token::Range(true));
                            i += 2;
                        } else {
                            tokens.push(Token::Range(false));
                            i += 1;
                        }
                    } else {
                        tokens.push(Token::Dot);
                    }
                }
                CharacterType::Operator(op) => {
                    let mut operator = String::new();
                    operator.push(op);
                    if (op == '>' || op == '<') && text[i + 1] == '=' {
                        operator.push('=');
                        i += 1;
                    }
                    tokens.push(Token::Operator(operator.into()));
                }
                CharacterType::Equal => {
                    if text[i + 1] == '=' {
                        i += 1;
                        tokens.push(Token::Operator("==".into()));
                    } else {
                        tokens.push(Token::Assign);
                    }
                }
                CharacterType::Empty => {}
                literal => tokens.push(match literal {
                    CharacterType::LParan => Token::LParan,
                    CharacterType::RParan => Token::RParan,
                    CharacterType::LBrace => Token::LBrace,
                    CharacterType::RBrace => Token::RBrace,
                    CharacterType::Comma => Token::Comma,
                    CharacterType::SemiColon => Token::SemiColon,
                    _ => unreachable!(),
                }),
            }
            i += 1;
        }

        tokens.reverse();
        Self { tokens }
    }
    pub fn pop(&mut self) -> Token {
        self.tokens.pop().unwrap_or(Token::EOF)
    }
    pub fn pop_more(&mut self, n: u32) {
        for _ in 0..n {
            self.tokens.pop();
        }
    }
    pub fn peek(&self) -> &Token {
        self.tokens
            .get(self.tokens.len() - 1)
            .unwrap_or(&Token::EOF)
    }
    pub fn peek_more(&self, more: u32) -> &Token {
        self.tokens
            .get(self.tokens.len() - 1 - more as usize)
            .unwrap_or(&Token::EOF)
    }
    pub fn is_eof(&self) -> bool {
        self.tokens.len() == 0
    }
}
impl Debug for TokenReader {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TokenReader[{}]",
            self.tokens
                .iter()
                .rev()
                .map(|token| format!("{:?}", token))
                .intersperse(",".to_string())
                .collect::<String>()
        )
    }
}
#[derive(Clone, Debug)]
pub enum Token {
    EOF,
    Int(i64),
    Float(f64),
    Identifier(ImmutableString),
    String(ImmutableString),
    LParan,
    RParan,
    LBrace,
    RBrace,
    Comma,
    SemiColon,
    Operator(ImmutableString),
    Dot,
    For,
    If,
    Fn,
    In,
    Range(bool),
    Assign,
}
#[derive(Copy, Clone)]
pub enum CharacterType {
    Alpha(char),
    Number(u8),
    Dot,
    LParan,
    RParan,
    LBrace,
    RBrace,
    Comma,
    SemiColon,
    Empty,
    Quote,
    Operator(char),
    Equal,
}
impl CharacterType {
    pub fn from_char(char: char) -> Self {
        match char {
            '"' => CharacterType::Quote,
            '.' => CharacterType::Dot,
            '(' => CharacterType::LParan,
            ')' => CharacterType::RParan,
            '{' => CharacterType::LBrace,
            '}' => CharacterType::RBrace,
            ',' => CharacterType::Comma,
            ';' => CharacterType::SemiColon,
            ' ' | '\t' | '\n' => CharacterType::Empty,
            '0'..='9' => CharacterType::Number(char as u8 - '0' as u8),
            '+' | '-' | '*' | '/' | '%' | '<' | '>' => CharacterType::Operator(char),
            '=' => CharacterType::Equal,
            _ => CharacterType::Alpha(char),
        }
    }
    pub fn as_alphanum(self) -> Option<char> {
        match self {
            CharacterType::Alpha(char) => Some(char),
            CharacterType::Number(char) => Some((char + '0' as u8) as char),
            _ => None,
        }
    }
}
