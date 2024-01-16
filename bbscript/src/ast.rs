use immutable_string::ImmutableString;
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
