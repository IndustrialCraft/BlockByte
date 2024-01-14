use immutable_string::ImmutableString;
use nom::character::complete::{alpha0, alphanumeric0};
use nom::error::ParseError;
use nom::multi::many0;
use nom::{AsChar, IResult, InputTakeAtPosition, Parser};
use std::error::Error;

pub struct StatementBlock {
    pub statements: Vec<Statement>,
}
pub enum Statement {
    Assign {
        is_let: bool,
        name: ImmutableString,
        value: Expression,
    },
    Eval {
        expression: Expression,
    },
    If {
        condition: Expression,
        satisfied: StatementBlock,
        unsatisfied: StatementBlock,
    },
}
pub enum Expression {
    StringLiteral {
        literal: ImmutableString,
    },
    IntLiteral {
        literal: i64,
    },
    UIntLiteral {
        literal: u64,
    },
    FloatLiteral {
        literal: f64,
    },
    Call {
        expression: Box<Expression>,
        parameters: Vec<Expression>,
    },
    MemberAccess {
        expression: Box<Expression>,
        name: ImmutableString,
    },
    ScopedVariable {
        name: ImmutableString,
    },
    CompareEquals {
        first: Box<Expression>,
        second: Box<Expression>,
        not_equals: bool,
    },
}
pub fn identifier<T, E: ParseError<T>>(input: T) -> IResult<T, T, E>
where
    T: InputTakeAtPosition,
    <T as InputTakeAtPosition>::Item: AsChar,
{
    input.split_at_position_complete(|item| {
        let item = item.as_char();
        !(item.is_alphanum() | (item == ':'))
    })
}
/*fn identifier<I: InputTakeAtPosition, E: ParseError<I>>() -> impl FnMut(I) -> IResult<I, I, E>
where
    <I as InputTakeAtPosition>::Item: AsChar,
{
    |input| alphanumeric0(input)
}*/
