use immutable_string::ImmutableString;

#[derive(Debug)]
pub struct StatementBlock {
    pub statements: Vec<Statement>,
}
#[derive(Debug)]
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
        unsatisfied: Option<StatementBlock>,
    },
    For {
        name: ImmutableString,
        expression: Expression,
        body: StatementBlock,
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
    Operator {
        first: Box<Expression>,
        second: Box<Expression>,
        operator: ImmutableString,
    },
}
