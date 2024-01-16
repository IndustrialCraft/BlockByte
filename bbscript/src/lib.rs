mod ast;
mod eval;
mod variant;

use lalrpop_util::lalrpop_mod;
lalrpop_mod!(pub syntax);

#[cfg(test)]
mod tests {
    use crate::ast;
    use crate::ast::{Expression, Statement, StatementBlock};
    use crate::eval::{ExecutionEnvironment, Function, ScriptError};
    use crate::variant::Variant;
    use std::cell::RefCell;

    #[test]
    fn test() {
        let mut environment = ExecutionEnvironment::new();
        environment
            .register_global_function("do_something", |params| Ok((Variant::String("aaa".into()))));
        environment.register_global_function("print", |params| {
            let output = params
                .into_iter()
                .map(|param| match param {
                    Variant::String(string) => Ok(string.to_string()),
                    _ => Err(()),
                })
                .collect::<Result<String, ()>>();
            match output {
                Ok(str) => {
                    println!("{str}");
                    Ok(Variant::Null)
                }
                Err(_) => Err(ScriptError::RuntimeError("cannot print non string".into())),
            }
        });
        Function {
            name: "aaa".into(),
            body: StatementBlock {
                statements: vec![Statement::Eval {
                    expression: Expression::Call {
                        expression: Box::new(Expression::ScopedVariable {
                            name: "print".into(),
                        }),
                        parameters: vec![Expression::Call {
                            expression: Box::new(Expression::ScopedVariable {
                                name: "do_something".into(),
                            }),
                            parameters: vec![],
                        }],
                    },
                }],
            },
            parameter_names: vec![],
        }
        .run(Variant::Null, vec![], &environment);

        println!(
            "{:?}",
            crate::syntax::ExpressionParser::new()
                .parse("ahoj.ako().sa.mas")
                .unwrap()
        );
    }
}
