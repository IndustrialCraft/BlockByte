#![feature(trait_upcasting)]
#![feature(iter_intersperse)]
#![feature(let_chains)]

pub mod ast;
pub mod environment;
pub mod eval;
pub mod lex;
pub mod variant;

use crate::eval::Function;
use immutable_string::ImmutableString;

pub fn parse_source_file(
    file: &str,
    file_name: Option<ImmutableString>,
    line_offset: u32,
) -> Result<Vec<Function>, Vec<String>> {
    let mut tokens = lex::TokenReader::lex(file_name, file, line_offset);
    let mut functions = Vec::new();
    while !tokens.is_eof() {
        functions.push(
            ast::parse_function(&mut tokens)
                .map_err(|error| vec![error])?
                .expect(format!("{tokens:?}").as_str()),
        );
    }
    Ok(functions)
}
