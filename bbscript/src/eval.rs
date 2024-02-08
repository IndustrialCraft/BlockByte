use crate::ast::{Expression, Statement, StatementBlock};
use crate::eval::ScriptError::{MismatchedParameters, VariableNotDefined};
use crate::variant::{FromVariant, FunctionType, FunctionVariant, IntoVariant, Primitive, Variant};
use immutable_string::ImmutableString;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum ScriptError {
    MismatchedParameters,
    VariableNotDefined,
    ConditionNotBool,
    MemberNotFound,
    NonFunctionCalled,
    RuntimeError(String),
    TypeError,
    NonVectorIterated,
}
pub type ScriptResult = Result<Variant, ScriptError>;

pub struct ScopeStack<'a> {
    previous: Option<&'a ScopeStack<'a>>,
    variables: Mutex<HashMap<ImmutableString, Variant>>,
}
impl<'a> ScopeStack<'a> {
    pub fn new() -> Self {
        ScopeStack {
            previous: None,
            variables: Mutex::new(HashMap::new()),
        }
    }
    pub fn push(&'a self) -> ScopeStack<'a> {
        ScopeStack {
            previous: Some(self),
            variables: Mutex::new(HashMap::new()),
        }
    }
    pub fn get_variable(&self, name: &str) -> Option<Variant> {
        if let Some(variable) = self.variables.lock().unwrap().get_mut(name) {
            return Some(variable.clone());
        }
        if let Some(previous) = &self.previous {
            return previous.get_variable(name);
        }
        return None;
    }
    pub fn set_variable(
        &self,
        name: ImmutableString,
        value: Variant,
        top: bool,
    ) -> Result<(), ScriptError> {
        if top {
            self.variables.lock().unwrap().insert(name, value);
        } else {
            if let Some(variable) = self.variables.lock().unwrap().get_mut(name.as_ref()) {
                *variable = value;
            } else {
                return if let Some(previous) = &self.previous {
                    previous.set_variable(name, value, false)
                } else {
                    Err(VariableNotDefined)
                };
            }
        }
        Ok(())
    }
}

pub type SharedFunction = Arc<Function>;

pub struct Function {
    pub name: ImmutableString,
    pub body: StatementBlock,
    pub parameter_names: Vec<ImmutableString>,
}
impl Function {
    pub fn run(
        &self,
        this: Variant,
        parameters: Vec<Variant>,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        let mut stack = ScopeStack::new();
        for (name, global) in &environment.globals {
            stack
                .set_variable(name.clone(), global.clone(), true)
                .unwrap();
        }
        stack.set_variable("this".into(), this, true).unwrap();
        if parameters.len() != self.parameter_names.len() {
            return Err(ScriptError::MismatchedParameters);
        }
        for (value, name) in parameters.into_iter().zip(self.parameter_names.iter()) {
            stack.set_variable(name.clone(), value, true).unwrap();
        }
        Function::execute_block(&mut stack, &self.body, environment)
    }
    fn execute_block(
        stack: &ScopeStack,
        block: &StatementBlock,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        let mut last_return_value = Variant::NULL();
        let stack = stack.push();
        for statement in &block.statements {
            last_return_value = match statement {
                Statement::Assign {
                    is_let,
                    name,
                    value,
                } => {
                    let value = Function::eval_expression(&stack, value, environment)?;
                    stack.set_variable(name.clone(), value, *is_let)?;
                    Variant::NULL()
                }
                Statement::Eval { expression } => {
                    Function::eval_expression(&stack, expression, environment)?
                }
                Statement::If {
                    condition,
                    satisfied,
                    unsatisfied,
                } => {
                    let expression = Function::eval_expression(&stack, condition, environment)?;
                    let statement =
                        if *bool::from_variant(&expression).ok_or(ScriptError::ConditionNotBool)? {
                            Some(satisfied)
                        } else {
                            unsatisfied.as_ref()
                        };
                    if let Some(statement) = statement {
                        Function::execute_block(&stack, statement, environment)?
                    } else {
                        Variant::NULL()
                    }
                }
                Statement::For {
                    expression,
                    name,
                    body,
                } => {
                    let expression = Function::eval_expression(&stack, expression, environment)?;
                    let stack = stack.push();
                    for value in Vec::<Variant>::from_variant(&expression)
                        .ok_or(ScriptError::NonVectorIterated)?
                    {
                        stack
                            .set_variable(name.clone(), value.clone(), true)
                            .unwrap();
                        Function::execute_block(&stack, body, environment)?;
                    }
                    Variant::NULL()
                }
            };
        }
        Ok(last_return_value)
    }
    fn eval_expression(
        stack: &ScopeStack,
        expression: &Expression,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        match expression {
            Expression::StringLiteral { literal } => Ok(literal.clone().into_variant()),
            Expression::IntLiteral { literal } => Ok((*literal).into_variant()),
            Expression::FloatLiteral { literal } => Ok((*literal).into_variant()),
            Expression::ScopedVariable { name } => {
                stack.get_variable(name.as_ref()).ok_or(VariableNotDefined)
            }
            Expression::Call {
                expression,
                parameters,
            } => {
                let expression = Function::eval_expression(stack, expression, environment)?;
                let parameters = parameters
                    .iter()
                    .map(|parameter| Function::eval_expression(stack, parameter, environment))
                    .collect::<Result<Vec<_>, ScriptError>>()?;
                Ok(expression.call(parameters, environment)?)
            }
            Expression::MemberAccess { expression, name } => {
                let value = Function::eval_expression(stack, expression, environment)?;
                environment
                    .access_member(&value, name)
                    .ok_or(ScriptError::MemberNotFound)
            }
            Expression::Operator {
                first,
                second,
                operator,
            } => {
                let first = Function::eval_expression(stack, first, environment)?;
                let second = Function::eval_expression(stack, second, environment)?;
                let operator_call = environment
                    .access_member(&first, &format!("operator{operator}").into())
                    .ok_or(ScriptError::MemberNotFound)?;
                Ok(operator_call.call(vec![second], environment)?)
            }
        }
    }
}
pub struct ExecutionEnvironment {
    types: HashMap<TypeId, TypeInfo>,
    globals: HashMap<ImmutableString, Variant>,
}
impl ExecutionEnvironment {
    pub fn new() -> Self {
        ExecutionEnvironment {
            types: HashMap::new(),
            globals: HashMap::new(),
        }
    }
    fn access_member(&self, value: &Variant, name: &ImmutableString) -> Option<Variant> {
        self.types
            .get(&((*value).type_id()))?
            .access_member(value, name)
    }
    pub fn register_member<
        T: Primitive,
        N: Into<ImmutableString>,
        F: Fn(&T) -> Option<Variant> + 'static,
    >(
        &mut self,
        name: N,
        function: F,
    ) {
        let function = Box::new(function);
        self.types
            .entry(TypeId::of::<T>())
            .or_insert(TypeInfo::new())
            .members
            .insert(
                name.into(),
                Box::new(move |this| function(T::from_variant(this).unwrap())),
            );
    }
    pub fn register_method<
        T: Primitive,
        F: IntoScriptFunction<T, A> + Send + Sync + 'static,
        N: Into<ImmutableString>,
        A: 'static,
    >(
        &mut self,
        name: N,
        function: F,
    ) {
        let function = function.into_method();
        let function = Arc::new(move |this: Variant, parameters| {
            function(T::from_variant(&this).unwrap(), parameters)
        });
        self.types
            .entry(TypeId::of::<T>())
            .or_insert(TypeInfo::new())
            .members
            .insert(
                name.into(),
                Box::new(move |this| {
                    Some(
                        FunctionVariant {
                            this: this.clone(),
                            function: FunctionType::RustFunction(function.clone()),
                        }
                        .into_variant(),
                    )
                }),
            );
    }
    pub fn register_global<F, N: Into<ImmutableString>>(&mut self, name: N, value: Variant) {
        self.globals.insert(name.into(), value);
    }
    pub fn register_function<F: IntoScriptFunction<(), A>, N: Into<ImmutableString>, A: 'static>(
        &mut self,
        name: N,
        function: F,
    ) {
        let function = function.into_function();
        let function = Arc::new(move |_: Variant, parameters| function(parameters));
        self.globals.insert(
            name.into(),
            FunctionVariant {
                this: Variant::NULL(),
                function: FunctionType::RustFunction(function),
            }
            .into_variant(),
        );
    }
}
pub struct TypeInfo {
    members: HashMap<ImmutableString, Box<dyn Fn(&Variant) -> Option<Variant>>>,
    default: Option<Box<dyn Fn(&Variant, ImmutableString) -> Option<Variant>>>,
}
impl TypeInfo {
    pub fn new() -> Self {
        TypeInfo {
            members: HashMap::new(),
            default: None,
        }
    }
    pub fn access_member(&self, value: &Variant, name: &ImmutableString) -> Option<Variant> {
        let variant =
            if let Some(value) = self.members.get(name).and_then(|function| function(value)) {
                Some(value)
            } else {
                self.default
                    .as_ref()
                    .and_then(|function| function(value, name.clone()))
            };
        if let Some(variant) = &variant {
            if let Some(function) = FunctionType::from_variant(variant) {
                return Some(
                    FunctionVariant {
                        this: value.clone(),
                        function: function.clone(),
                    }
                    .into_variant(),
                );
            }
        }
        variant
    }
}

pub trait IntoScriptFunction<S: Primitive, A: 'static> {
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync>;
    fn into_method(self) -> Box<dyn Fn(&S, Vec<Variant>) -> ScriptResult + Send + Sync>;
}
impl<T, A> IntoScriptFunction<A, (A,)> for T
where
    A: Primitive,
    T: Fn(&A) -> ScriptResult + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 1 {
                return Err(MismatchedParameters);
            }
            self(A::from_variant(&args[0]).ok_or(MismatchedParameters)?)
        })
    }
    fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |this, args| {
            if args.len() != 0 {
                return Err(MismatchedParameters);
            }
            self(this)
        })
    }
}
impl<T, A, B> IntoScriptFunction<A, (A, B)> for T
where
    A: Primitive,
    B: Primitive,
    T: Fn(&A, &B) -> ScriptResult + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 2 {
                return Err(MismatchedParameters);
            }
            self(
                A::from_variant(&args[0]).ok_or(MismatchedParameters)?,
                B::from_variant(&args[1]).ok_or(MismatchedParameters)?,
            )
        })
    }
    fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |this, args| {
            if args.len() != 1 {
                return Err(MismatchedParameters);
            }
            self(this, B::from_variant(&args[0]).ok_or(MismatchedParameters)?)
        })
    }
}
impl<T, A, B, C> IntoScriptFunction<A, (A, B, C)> for T
where
    A: Primitive,
    B: Primitive,
    C: Primitive,
    T: Fn(&A, &B, &C) -> ScriptResult + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 3 {
                return Err(MismatchedParameters);
            }
            self(
                A::from_variant(&args[0]).ok_or(MismatchedParameters)?,
                B::from_variant(&args[1]).ok_or(MismatchedParameters)?,
                C::from_variant(&args[2]).ok_or(MismatchedParameters)?,
            )
        })
    }
    fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |this, args| {
            if args.len() != 2 {
                return Err(MismatchedParameters);
            }
            self(
                this,
                B::from_variant(&args[0]).ok_or(MismatchedParameters)?,
                C::from_variant(&args[1]).ok_or(MismatchedParameters)?,
            )
        })
    }
}
