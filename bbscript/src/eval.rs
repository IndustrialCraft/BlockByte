use crate::ast::{Expression, Statement, StatementBlock};
use crate::variant::{FunctionType, Variant};
use immutable_string::ImmutableString;
use std::any::{Any, TypeId};
use std::collections::HashMap;

pub enum ScriptError {
    MismatchedParameters,
    VariableNotDefined,
    ConditionNotBool,
    MemberNotFound,
    NonFunctionCalled,
}
pub type ScriptResult = Result<Variant, ScriptError>;

pub struct ScopeStack {
    stack: Vec<HashMap<ImmutableString, Variant>>,
}
impl ScopeStack {
    pub fn new() -> Self {
        ScopeStack { stack: Vec::new() }
    }
    pub fn push(&mut self) {
        self.stack.push(HashMap::new());
    }
    pub fn pop(&mut self) {
        self.stack.pop().expect("empty scope stack was popped");
    }
    pub fn get_variable_mut(&mut self, name: &str) -> Option<&mut Variant> {
        for scope in self.stack.iter_mut().rev() {
            if let Some(variant) = scope.get_mut(name) {
                return Some(variant);
            }
        }
        None
    }
    pub fn set_variable(&mut self, name: ImmutableString, value: Variant) {
        self.stack.last_mut().unwrap().insert(name, value);
    }
}

pub struct Function {
    name: ImmutableString,
    body: StatementBlock,
    parameter_names: Vec<ImmutableString>,
}
impl Function {
    pub fn run(
        &self,
        this: Variant,
        parameters: Vec<Variant>,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        let mut stack = ScopeStack::new();
        stack.push();
        for (name, global) in &environment.globals {
            stack.set_variable(name.clone(), global.clone());
        }
        stack.set_variable("this".into(), this);
        if parameters.len() != self.parameter_names.len() {
            return Err(ScriptError::MismatchedParameters);
        }
        for (value, name) in parameters.into_iter().zip(self.parameter_names.iter()) {
            stack.set_variable(name.clone(), value);
        }
        Function::execute_block(&mut stack, &self.body, environment)
    }
    fn execute_block(
        stack: &mut ScopeStack,
        block: &StatementBlock,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        let mut last_return_value = Variant::Null;
        stack.push();
        for statement in &block.statements {
            last_return_value = match statement {
                Statement::Assign {
                    is_let,
                    name,
                    value,
                } => {
                    let value = Function::eval_expression(stack, value, environment)?;
                    if *is_let {
                        stack.set_variable(name.clone(), value);
                    } else {
                        *stack
                            .get_variable_mut(name.as_ref())
                            .ok_or(ScriptError::VariableNotDefined)? = value;
                    }
                    Variant::Null
                }
                Statement::Eval { expression } => {
                    Function::eval_expression(stack, expression, environment)?
                }
                Statement::If {
                    condition,
                    satisfied,
                    unsatisfied,
                } => match Function::eval_expression(stack, condition, environment)? {
                    Variant::Bool(result) => Function::execute_block(
                        stack,
                        if result { satisfied } else { unsatisfied },
                        environment,
                    )?,
                    _ => return Err(ScriptError::ConditionNotBool),
                },
            };
        }
        stack.pop();
        Ok(last_return_value)
    }
    fn eval_expression(
        stack: &mut ScopeStack,
        expression: &Expression,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        match expression {
            Expression::StringLiteral { literal } => Ok(Variant::String(literal.clone())),
            Expression::IntLiteral { literal } => Ok(Variant::Int(*literal)),
            Expression::UIntLiteral { literal } => Ok(Variant::UInt(*literal)),
            Expression::FloatLiteral { literal } => Ok(Variant::Float(*literal)),
            Expression::ScopedVariable { name } => stack
                .get_variable_mut(name.as_ref())
                .cloned()
                .ok_or(ScriptError::VariableNotDefined),
            Expression::Call {
                expression,
                parameters,
            } => {
                let expression = Function::eval_expression(stack, expression, environment)?;
                let parameters = parameters
                    .iter()
                    .map(|parameter| Function::eval_expression(stack, parameter, environment))
                    .collect::<Result<Vec<_>, ScriptError>>()?;
                match expression {
                    Variant::Function(this, function) => match function {
                        FunctionType::ScriptFunction(function) => {
                            function.run(*this, parameters, environment)
                        }
                        FunctionType::RustFunction(function) => function(*this, parameters),
                    },
                    _ => return Err(ScriptError::NonFunctionCalled),
                }
            }
            Expression::MemberAccess { expression, name } => {
                let value = Function::eval_expression(stack, expression, environment)?;
                environment
                    .access_member(&value, name)
                    .ok_or(ScriptError::MemberNotFound)
            }
            Expression::CompareEquals {
                first,
                second,
                not_equals,
            } => Ok(Variant::Bool(
                (Function::eval_expression(stack, first, environment)?
                    == Function::eval_expression(stack, second, environment)?)
                    ^ not_equals,
            )),
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
            .get(&value.get_type())?
            .access_member(value, name)
    }
    pub fn register_member<F: 'static, N: Into<ImmutableString>>(
        &mut self,
        name: N,
        function: Box<dyn Fn(&F) -> Option<Variant>>,
    ) {
        self.types
            .entry(TypeId::of::<F>())
            .or_insert(TypeInfo::new())
            .members
            .insert(
                name.into(),
                Box::new(move |this| function(this.get_ref().downcast_ref().unwrap())),
            );
    }
    pub fn register_function<F: 'static, N: Into<ImmutableString>>(
        &mut self,
        name: N,
        value: FunctionType,
    ) {
        self.types
            .entry(TypeId::of::<F>())
            .or_insert(TypeInfo::new())
            .members
            .insert(
                name.into(),
                Box::new(move |this| {
                    Some(Variant::Function(Box::new(this.clone()), value.clone()))
                }),
            );
    }
    pub fn register_global(&mut self, name: ImmutableString, value: Variant) {
        self.globals.insert(name, value);
    }
}
pub struct TypeInfo {
    members: HashMap<ImmutableString, Box<dyn Fn(&Variant) -> Option<Variant>>>,
}
impl TypeInfo {
    pub fn new() -> Self {
        TypeInfo {
            members: HashMap::new(),
        }
    }
    pub fn access_member(&self, value: &Variant, name: &ImmutableString) -> Option<Variant> {
        self.members.get(name)?(value)
    }
}
