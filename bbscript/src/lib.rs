#![feature(trait_upcasting)]

pub mod ast;
pub mod environment;
pub mod eval;
pub mod variant;

use crate::eval::Function;
use lalrpop_util::lexer::Token;
use lalrpop_util::{lalrpop_mod, ErrorRecovery, ParseError};
lalrpop_mod!(pub syntax);

pub fn parse_source_file(file: &str, line_offset: u32) -> Result<Vec<Function>, Vec<String>> {
    let mut errors: Vec<ErrorRecovery<usize, Token, &'static str>> = Vec::new();
    let result = crate::syntax::SourceFileParser::new().parse(&mut errors, file);
    let mut errors: Vec<_> = errors.into_iter().map(|error| error.error).collect();
    if let Err(error) = &result {
        errors.push(error.clone());
    }
    if errors.len() >= 1 {
        Err(errors
            .into_iter()
            .map(|error| {
                let (line, col) = find_line_col(
                    file,
                    match &error {
                        ParseError::InvalidToken { location } => *location,
                        ParseError::UnrecognizedEof { location, .. } => *location,
                        ParseError::UnrecognizedToken { token, .. } => token.0,
                        ParseError::ExtraToken { token } => token.0,
                        ParseError::User { .. } => unreachable!(),
                    },
                );
                format!("{}:{col}-{}", line + line_offset, error.to_string())
            })
            .collect())
    } else {
        Ok(result.unwrap())
    }
}
fn find_line_col(file: &str, token: usize) -> (u32, u32) {
    let mut line = 1;
    let mut col = 0;
    for (i, char) in file.chars().enumerate() {
        if char == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        if token == i {
            return (line, col);
        }
    }
    panic!()
}
