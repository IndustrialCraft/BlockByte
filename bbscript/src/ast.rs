use crate::eval::Function;
use immutable_string::ImmutableString;
use std::sync::Arc;

#[derive(Debug)]
pub struct StatementBlock {
    pub statements: Vec<Statement>,
}
#[derive(Debug)]
pub enum Statement {
    Assign {
        is_let: bool,
        operator: Option<ImmutableString>,
        left: Expression,
        value: Expression,
    },
    Eval {
        expression: Expression,
    },
    If {
        condition: Expression,
        satisfied: StatementBlock,
        unsatisfied: Option<StatementBlock>,
    },
    For {
        name: ImmutableString,
        expression: Expression,
        body: StatementBlock,
    },
    Return {
        expression: Option<Expression>,
    },
    Break {
        expression: Option<Expression>,
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
    RangeLiteral {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    FunctionLiteral {
        function: Arc<Function>,
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
    Operator {
        first: Box<Expression>,
        second: Box<Expression>,
        operator: ImmutableString,
    },
    UnaryOperator {
        expression: Box<Expression>,
        operator: ImmutableString,
    },
    Error,
}
