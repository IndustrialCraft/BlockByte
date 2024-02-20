use crate::eval::Function;
use crate::lex::{Token, TokenReader};
use immutable_string::ImmutableString;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct StatementBlock {
    pub statements: Vec<Statement>,
}
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
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

pub fn parse_expression(tokens: &mut TokenReader) -> Option<Expression> {
    let mut left_side = None;
    match (tokens.peek(), tokens.peek_more(1), tokens.peek_more(2)) {
        (Token::Int(start), Token::Range(inclusive), Token::Int(end)) => {
            left_side = Some(Expression::RangeLiteral {
                start: *start,
                end: *end,
                inclusive: *inclusive,
            });
            tokens.pop_more(3);
        }
        _ => {}
    }
    loop {
        match (&mut left_side, tokens.peek().clone()) {
            (None, Token::Operator(operator)) => {
                tokens.pop();
                left_side = Some(Expression::UnaryOperator {
                    expression: Box::new(parse_expression(tokens).unwrap()),
                    operator,
                });
            }
            (None, Token::Identifier(name)) => {
                left_side = Some(Expression::ScopedVariable { name });
                tokens.pop();
            }
            (None, Token::String(literal)) => {
                left_side = Some(Expression::StringLiteral { literal });
                tokens.pop();
            }
            (None, Token::Int(literal)) => {
                left_side = Some(Expression::IntLiteral { literal });
                tokens.pop();
            }
            (None, Token::Float(literal)) => {
                left_side = Some(Expression::FloatLiteral { literal });
                tokens.pop();
            }
            (Some(expression), Token::Dot) => {
                tokens.pop();
                let name = match tokens.pop() {
                    Token::Identifier(name) => name,
                    _ => panic!(),
                };
                *expression = Expression::MemberAccess {
                    expression: Box::new(expression.clone()),
                    name,
                };
            }
            (Some(expression), Token::LParan) => {
                tokens.pop();
                let mut parameters = Vec::new();
                let mut skip = false;
                while let Some(param) = parse_expression(tokens) {
                    parameters.push(param);
                    match tokens.pop() {
                        Token::Comma => {}
                        Token::RParan => {
                            skip = true;
                            break;
                        }
                        _ => panic!(),
                    }
                }
                if !skip {
                    match tokens.pop() {
                        Token::RParan => {}
                        _ => panic!(),
                    }
                }
                *expression = Expression::Call {
                    expression: Box::new(expression.clone()),
                    parameters,
                };
            }
            (Some(expression), Token::Operator(operator)) => {
                tokens.pop();
                let second = parse_expression(tokens).unwrap();
                *expression = Expression::Operator {
                    first: Box::new(expression.clone()),
                    operator,
                    second: Box::new(second),
                };
            }
            _ => return left_side,
        }
    }
}
