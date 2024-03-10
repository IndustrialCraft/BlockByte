use immutable_string::ImmutableString;
use std::fmt::{Debug, Formatter};
use std::str::FromStr;

pub struct TokenReader {
    tokens: Vec<(Token, FilePosition)>,
}
impl TokenReader {
    pub fn lex(file_name: Option<ImmutableString>, text: &str, line_offset: u32) -> TokenReader {
        let line_info = LineInfo::new(text);
        let text: Vec<char> = text.chars().collect();
        let mut tokens = Vec::new();

        let mut i = 0;
        loop {
            if i >= text.len() {
                break;
            }
            let token_start = i;
            let mut add_token = |token| {
                let mut position =
                    FilePosition::new(file_name.clone(), token_start as u32, &line_info);
                position.line += line_offset;
                tokens.push((token, position))
            };

            if text[i] == '/' && text[i + 1] == '/' {
                while text[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if text[i] == '/' && text[i + 1] == '*' {
                while text[i] != '*' && text[i + 1] != '/' {
                    i += 1;
                }
                continue;
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
                    add_token(match string.as_str() {
                        "fn" => Token::Fn,
                        "for" => Token::For,
                        "if" => Token::If,
                        "in" => Token::In,
                        "let" => Token::Let,
                        "else" => Token::Else,
                        "return" => Token::Return,
                        "break" => Token::Break,
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
                        add_token(Token::Float(f64::from_str(numbers.as_str()).unwrap()))
                    } else {
                        add_token(Token::Int(i64::from_str(numbers.as_str()).unwrap()))
                    }
                }
                CharacterType::Quote => {
                    let mut string = String::new();
                    while text[i + 1] != '"' {
                        string.push(text[i + 1]);
                        i += 1;
                    }
                    i += 1;
                    add_token(Token::String(string.into()));
                }
                CharacterType::Dot => {
                    if text[i + 1] == '.' {
                        if text[i + 2] == '=' {
                            add_token(Token::Range(true));
                            i += 2;
                        } else {
                            add_token(Token::Range(false));
                            i += 1;
                        }
                    } else {
                        add_token(Token::Dot);
                    }
                }
                CharacterType::Operator(op) => {
                    if text[i + 1] == '=' {
                        i += 1;
                        if op == '!' || op == '<' || op == '>' {
                            add_token(Token::Operator(format!("{op}=").into()));
                        } else {
                            add_token(Token::Assign(Some(format!("{op}").into())));
                        }
                    } else {
                        add_token(Token::Operator(format!("{op}").into()));
                    }
                }
                CharacterType::Equal => {
                    if text[i + 1] == '=' {
                        i += 1;
                        add_token(Token::Operator("==".into()));
                    } else {
                        add_token(Token::Assign(None));
                    }
                }
                CharacterType::Empty => {}
                literal => add_token(match literal {
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
    pub fn pop_assert(&mut self, expected: Token) -> Result<(), String> {
        let (token, position) = self.pop();
        if token != expected {
            Err(format!(
                "expected {expected:?}, got {token:?} at {position:?}"
            ))
        } else {
            Ok(())
        }
    }
    pub fn assert_identifier(&mut self) -> Result<ImmutableString, String> {
        let (token, position) = self.pop();
        match token {
            Token::Identifier(identifier) => Ok(identifier),
            _ => Err(format!(
                "expected Identifier, got {token:?} at {position:?}"
            )),
        }
    }
    pub fn pop(&mut self) -> (Token, FilePosition) {
        self.tokens
            .pop()
            .unwrap_or((Token::EOF, FilePosition::INVALID))
    }
    pub fn peek(&self) -> &Token {
        self.tokens
            .get(self.tokens.len() - 1)
            .map(|token| &token.0)
            .unwrap_or(&Token::EOF)
    }
    pub fn peek_offset(&self, more: u32) -> &Token {
        self.tokens
            .get(self.tokens.len() - 1 - more as usize)
            .map(|token| &token.0)
            .unwrap_or(&Token::EOF)
    }
    pub fn get_position(&self) -> FilePosition {
        self.tokens
            .get(0)
            .map(|token| token.1.clone())
            .unwrap_or(FilePosition::INVALID)
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
                .map(|token| format!("{:?},", token))
                .collect::<String>()
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
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
    Let,
    Else,
    Return,
    Break,
    Range(bool),
    Assign(Option<ImmutableString>),
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
            '+' | '-' | '*' | '/' | '%' | '<' | '>' | '!' => CharacterType::Operator(char),
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
pub struct LineInfo {
    lines: Vec<u32>,
}
impl LineInfo {
    pub fn new(file: &str) -> Self {
        LineInfo {
            lines: file.split("\n").map(|part| part.len() as u32 + 1).collect(),
        }
    }
}
#[derive(Clone)]
pub struct FilePosition {
    pub file: Option<ImmutableString>,
    pub line: u32,
    pub offset: u32,
}
impl FilePosition {
    pub const INVALID: FilePosition = FilePosition {
        file: None,
        line: 0,
        offset: 0,
    };
    pub fn new(file: Option<ImmutableString>, mut offset: u32, line_info: &LineInfo) -> Self {
        let mut line = 1;

        for line_length in &line_info.lines {
            if offset < *line_length {
                break;
            }
            offset -= line_length;
            line += 1;
        }

        FilePosition { file, line, offset }
    }
}
impl Debug for FilePosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "[{}:{}:{}]", file, self.line, self.offset)
        } else {
            write!(f, "[{}:{}]", self.line, self.offset)
        }
    }
}
