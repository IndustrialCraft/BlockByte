#![feature(trait_upcasting)]

pub mod ast;
pub mod environment;
pub mod eval;
pub mod variant;

use crate::eval::Function;
use lalrpop_util::lalrpop_mod;
lalrpop_mod!(pub syntax);

pub fn parse_source_file(file: &str) -> Result<Vec<Function>, String> {
    crate::syntax::SourceFileParser::new()
        .parse(file)
        .map_err(|error| error.to_string())
}
