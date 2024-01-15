use immutable_string::ImmutableString;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::{alpha0, alpha1, alphanumeric0, alphanumeric1, digit0, digit1};
use nom::combinator::value;
use nom::error::{ErrorKind, ParseError};
use nom::multi::{many0, separated_list0};
use nom::sequence::tuple;
use nom::Err::Error;
use nom::{AsChar, IResult, InputTakeAtPosition, Parser};
use nom_locate::LocatedSpan;
use nom_recursive::{recursive_parser, RecursiveInfo};
use std::cell::RefCell;

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
#[derive(Debug)]
pub enum Expression {
    StringLiteral {
        literal: ImmutableString,
    },
    IntLiteral {
        literal: i64,
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
    Operator {
        first: Box<Expression>,
        second: Box<Expression>,
        operator: ImmutableString,
    },
}
type Span<'a> = LocatedSpan<&'a str, u32>;

fn string_literal(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = tuple((tag("\""), alphanumeric0, tag("\"")))(input)?;
    Ok((
        remaining,
        Expression::StringLiteral {
            literal: parsed.1.to_string().into(),
        },
    ))
}
fn int_literal(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = digit1(input)?;
    Ok((
        remaining,
        Expression::IntLiteral {
            literal: parsed.parse().unwrap(),
        },
    ))
}
fn float_literal(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = tuple((digit1, tag("."), digit0))(input)?;
    Ok((
        remaining,
        Expression::FloatLiteral {
            literal: format!("{}.{}", parsed.0, parsed.2).parse().unwrap(),
        },
    ))
}
fn call_expression(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = tuple((
        expression,
        tag("("),
        separated_list0(tag(","), expression),
        tag(")"),
    ))(input)?;
    Ok((
        remaining,
        Expression::Call {
            expression: Box::new(parsed.0),
            parameters: parsed.2,
        },
    ))
}
fn member_access(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = tuple((expression, tag("."), alphanumeric1))(input)?;
    Ok((
        remaining,
        Expression::MemberAccess {
            expression: Box::new(parsed.0),
            name: parsed.2.to_string().into(),
        },
    ))
}
fn scoped_variable(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = tuple((alpha1, alphanumeric0))(input)?;
    Ok((
        remaining,
        Expression::ScopedVariable {
            name: format!("{}{}", parsed.0, parsed.1).into(),
        },
    ))
}
fn compare_equals(input: Span) -> IResult<Span, Expression> {
    let (remaining, parsed) = tuple((
        expression,
        alt((value(false, tag("==")), value(true, tag("!=")))),
        expression,
    ))(input)?;
    Ok((
        remaining,
        Expression::CompareEquals {
            first: Box::new(parsed.0),
            second: Box::new(parsed.2),
            not_equals: parsed.1,
        },
    ))
}
pub fn expression(s: Span) -> IResult<Span, Expression> {
    if s.extra >= 5 {
        return Err(nom::Err::Error(nom::error::Error::new(s, ErrorKind::Fail)));
    }
    let mut new_span = s.clone();
    new_span.extra += 1;
    let result = alt((
        string_literal,
        float_literal,
        int_literal,
        call_expression,
        member_access,
        compare_equals,
        scoped_variable,
    ))(new_span);
    result
}
