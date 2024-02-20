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
    if let Some(function) = parse_function(tokens) && left_side.is_none(){
        left_side = Some(Expression::FunctionLiteral {function: Arc::new(function)});
    }
    loop {
        match (&mut left_side, tokens.peek().clone()) {
            (None, Token::LParan) => {
                tokens.pop();
                let expression = parse_expression(tokens).unwrap();
                match tokens.pop() {
                    Token::RParan => {}
                    _ => panic!(),
                }
                left_side = Some(expression);
            }
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
pub fn parse_statement(tokens: &mut TokenReader) -> Option<Statement> {
    match tokens.peek().clone() {
        Token::If => {
            tokens.pop();
            let condition = parse_expression(tokens).unwrap();
            let satisfied = parse_statement_block(tokens);
            let unsatisfied = if tokens.peek() == &Token::Else {
                tokens.pop();
                Some(parse_statement_block(tokens))
            } else {
                None
            };
            Some(Statement::If {
                condition,
                satisfied,
                unsatisfied,
            })
        }
        Token::For => {
            tokens.pop();
            let name = match tokens.pop() {
                Token::Identifier(name) => name,
                _ => panic!(),
            };
            match tokens.pop() {
                Token::In => {}
                _ => panic!(),
            }
            let expression = parse_expression(tokens).unwrap();
            let body = parse_statement_block(tokens);
            Some(Statement::For {
                name,
                expression,
                body,
            })
        }
        Token::Return | Token::Break => {
            let token = tokens.pop();
            let expression = parse_expression(tokens);
            match tokens.pop() {
                Token::SemiColon => {}
                _ => panic!(),
            }
            Some(match token {
                Token::Return => Statement::Return { expression },
                Token::Break => Statement::Break { expression },
                _ => unreachable!(),
            })
        }
        _ => {
            let is_let = tokens.peek() == &Token::Let;
            if is_let {
                tokens.pop();
            }
            let left = parse_expression(tokens)?;
            let operator = match tokens.pop() {
                Token::Assign(operator) => operator,
                Token::SemiColon => return Some(Statement::Eval { expression: left }),
                _ => panic!(),
            };
            let right = parse_expression(tokens).unwrap();
            match tokens.pop() {
                Token::SemiColon => {}
                _ => panic!(),
            }
            Some(Statement::Assign {
                is_let,
                left,
                value: right,
                operator,
            })
        }
    }
}
pub fn parse_statement_block(tokens: &mut TokenReader) -> StatementBlock {
    match tokens.pop() {
        Token::LBrace => {}
        _ => panic!(),
    }
    let mut statements = Vec::new();
    while let Some(statement) = parse_statement(tokens) {
        statements.push(statement);
    }
    match tokens.pop() {
        Token::RBrace => {}
        _ => panic!(),
    }
    StatementBlock { statements }
}
pub fn parse_function(tokens: &mut TokenReader) -> Option<Function> {
    match tokens.peek() {
        Token::Fn => {
            tokens.pop();
        }
        _ => return None,
    }
    let name = match tokens.peek().clone() {
        Token::Identifier(name) => {
            tokens.pop();
            name
        }
        _ => "anon".into(),
    };
    match tokens.peek() {
        Token::LParan => {
            tokens.pop();
        }
        _ => panic!(),
    }
    let mut parameter_names = Vec::new();
    loop {
        match tokens.pop() {
            Token::Identifier(arg) => {
                parameter_names.push(arg);
                match tokens.pop() {
                    Token::RParan => break,
                    Token::Comma => {}
                    _ => panic!(),
                }
            }
            Token::RParan => break,
            _ => panic!(),
        }
    }
    let body = parse_statement_block(tokens);

    Some(Function {
        name,
        parameter_names,
        body,
    })
}
