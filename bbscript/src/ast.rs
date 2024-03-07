use crate::eval::Function;
use crate::lex::{FilePosition, Token, TokenReader};
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
        position: FilePosition,
        literal: ImmutableString,
    },
    IntLiteral {
        position: FilePosition,
        literal: i64,
    },
    FloatLiteral {
        position: FilePosition,
        literal: f64,
    },
    RangeLiteral {
        position: FilePosition,
        start: i64,
        end: i64,
        inclusive: bool,
    },
    FunctionLiteral {
        position: FilePosition,
        function: Arc<Function>,
    },
    Call {
        position: FilePosition,
        expression: Box<Expression>,
        parameters: Vec<Expression>,
    },
    MemberAccess {
        position: FilePosition,
        expression: Box<Expression>,
        name: ImmutableString,
    },
    ScopedVariable {
        position: FilePosition,
        name: ImmutableString,
    },
    Operator {
        position: FilePosition,
        first: Box<Expression>,
        second: Box<Expression>,
        operator: ImmutableString,
    },
    UnaryOperator {
        position: FilePosition,
        expression: Box<Expression>,
        operator: ImmutableString,
    },
}
impl Expression {
    pub fn get_file_position(&self) -> &FilePosition {
        match self {
            Expression::StringLiteral { position, .. } => position,
            Expression::IntLiteral { position, .. } => position,
            Expression::FloatLiteral { position, .. } => position,
            Expression::RangeLiteral { position, .. } => position,
            Expression::FunctionLiteral { position, .. } => position,
            Expression::Call { position, .. } => position,
            Expression::MemberAccess { position, .. } => position,
            Expression::ScopedVariable { position, .. } => position,
            Expression::Operator { position, .. } => position,
            Expression::UnaryOperator { position, .. } => position,
        }
    }
}

pub fn parse_expression(tokens: &mut TokenReader) -> Result<Option<Expression>, String> {
    let mut left_side = None;
    match (tokens.peek(), tokens.peek_offset(1), tokens.peek_offset(2)) {
        (Token::Int(start), Token::Range(inclusive), Token::Int(end)) => {
            left_side = Some(Expression::RangeLiteral {
                start: *start,
                end: *end,
                inclusive: *inclusive,
                position: tokens.pop().1,
            });
            tokens.pop();
            tokens.pop();
        }
        _ => {}
    }
    {
        let position = tokens.get_position();
        if let Some(function) = parse_function(tokens)? {
            if left_side.is_none() {
                left_side = Some(Expression::FunctionLiteral {
                    position,
                    function: Arc::new(function),
                });
            }
        }
    }
    loop {
        match (&mut left_side, tokens.peek().clone()) {
            (None, Token::LParan) => {
                tokens.pop();
                let expression = parse_expression(tokens)?.unwrap();
                tokens.pop_assert(Token::RParan)?;
                left_side = Some(expression);
            }
            (None, Token::Operator(operator)) => {
                //todo: precedence
                left_side = Some(Expression::UnaryOperator {
                    position: tokens.pop().1,
                    expression: Box::new(parse_expression(tokens)?.unwrap()),
                    operator,
                });
            }
            (None, Token::Identifier(name)) => {
                left_side = Some(Expression::ScopedVariable {
                    position: tokens.pop().1,
                    name,
                });
            }
            (None, Token::String(literal)) => {
                left_side = Some(Expression::StringLiteral {
                    position: tokens.pop().1,
                    literal,
                });
            }
            (None, Token::Int(literal)) => {
                left_side = Some(Expression::IntLiteral {
                    position: tokens.pop().1,
                    literal,
                });
            }
            (None, Token::Float(literal)) => {
                left_side = Some(Expression::FloatLiteral {
                    position: tokens.pop().1,
                    literal,
                });
            }
            (Some(expression), Token::Dot) => {
                let position = tokens.pop().1;
                let name = tokens.assert_identifier()?;
                *expression = Expression::MemberAccess {
                    position,
                    expression: Box::new(expression.clone()),
                    name,
                };
            }
            (Some(expression), Token::LParan) => {
                let position = tokens.pop().1;
                let mut parameters = Vec::new();
                let mut skip = false;
                while let Some(param) = parse_expression(tokens)? {
                    parameters.push(param);
                    match tokens.pop().0 {
                        Token::Comma => {}
                        Token::RParan => {
                            skip = true;
                            break;
                        }
                        token => panic!("{:?}", token),
                    }
                }
                if !skip {
                    tokens.pop_assert(Token::RParan)?;
                }
                *expression = Expression::Call {
                    position,
                    expression: Box::new(expression.clone()),
                    parameters,
                };
            }
            (Some(expression), Token::Operator(operator)) => {
                let position = tokens.pop().1;
                let second = parse_expression(tokens)?.unwrap();
                *expression = Expression::Operator {
                    position,
                    first: Box::new(expression.clone()),
                    operator,
                    second: Box::new(second),
                };
            }
            _ => return Ok(left_side),
        }
    }
}
pub fn parse_statement(tokens: &mut TokenReader) -> Result<Option<Statement>, String> {
    match tokens.peek().clone() {
        Token::If => {
            tokens.pop();
            let condition = parse_expression(tokens)?.unwrap();
            let satisfied = parse_statement_block(tokens)?;
            let unsatisfied = if tokens.peek() == &Token::Else {
                tokens.pop();
                Some(parse_statement_block(tokens)?)
            } else {
                None
            };
            Ok(Some(Statement::If {
                condition,
                satisfied,
                unsatisfied,
            }))
        }
        Token::For => {
            tokens.pop();
            let name = tokens.assert_identifier()?;
            tokens.pop_assert(Token::In)?;
            let expression = parse_expression(tokens)?.unwrap();
            let body = parse_statement_block(tokens)?;
            Ok(Some(Statement::For {
                name,
                expression,
                body,
            }))
        }
        Token::Return | Token::Break => {
            let token = tokens.pop().0;
            let expression = parse_expression(tokens)?;
            tokens.pop_assert(Token::SemiColon)?;
            Ok(Some(match token {
                Token::Return => Statement::Return { expression },
                Token::Break => Statement::Break { expression },
                _ => unreachable!(),
            }))
        }
        _ => {
            let is_let = tokens.peek() == &Token::Let;
            if is_let {
                tokens.pop();
            }
            let left = match parse_expression(tokens)? {
                Some(left) => left,
                None => return Ok(None),
            };
            let operator = match tokens.pop().0 {
                Token::Assign(operator) => operator,
                Token::SemiColon => return Ok(Some(Statement::Eval { expression: left })),
                _ => panic!(),
            };
            let right = parse_expression(tokens)?.unwrap();
            tokens.pop_assert(Token::SemiColon)?;
            Ok(Some(Statement::Assign {
                is_let,
                left,
                value: right,
                operator,
            }))
        }
    }
}
pub fn parse_statement_block(tokens: &mut TokenReader) -> Result<StatementBlock, String> {
    tokens.pop_assert(Token::LBrace)?;
    let mut statements = Vec::new();
    while let Some(statement) = parse_statement(tokens)? {
        statements.push(statement);
    }
    tokens.pop_assert(Token::RBrace)?;
    Ok(StatementBlock { statements })
}
pub fn parse_function(tokens: &mut TokenReader) -> Result<Option<Function>, String> {
    match tokens.peek() {
        Token::Fn => {
            tokens.pop();
        }
        _ => return Ok(None),
    }
    let name = match tokens.peek().clone() {
        Token::Identifier(name) => {
            tokens.pop();
            name
        }
        _ => "anon".into(),
    };
    tokens.pop_assert(Token::LParan)?;
    let mut parameter_names = Vec::new();
    loop {
        match tokens.pop().0 {
            Token::Identifier(arg) => {
                parameter_names.push(arg);
                match tokens.pop().0 {
                    Token::RParan => break,
                    Token::Comma => {}
                    _ => panic!(),
                }
            }
            Token::RParan => break,
            _ => panic!(),
        }
    }
    let body = parse_statement_block(tokens)?;

    Ok(Some(Function {
        name,
        parameter_names,
        body,
    }))
}
