use crate::ast::{Expression, Statement, StatementBlock};
use crate::eval::ScriptError::{BreakOutsideLoop, InvalidIterator, MemberNotFound};
use crate::lex::FilePosition;
use crate::variant::{
    FromVariant, FunctionType, FunctionVariant, IntoVariant, Primitive, SharedArray, TypeName,
    Variant,
};
use immutable_string::ImmutableString;
use parking_lot::Mutex;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::ops::{ControlFlow, FromResidual, Range, Try};
use std::sync::Arc;

#[derive(Debug)]
pub enum ScriptError {
    MismatchedParameterCount {
        position: FilePosition,
        function_name: String,
        expected: Vec<String>,
        got: Vec<TypeName>,
    },
    MismatchedType {
        position: FilePosition,
        expected: TypeName,
        got: TypeName,
    },
    VariableNotDefined {
        position: FilePosition,
        variable: String,
    },
    BreakOutsideLoop,
    MemberNotFound {
        position: FilePosition,
        member: String,
    },
    NonFunctionCalled {
        position: FilePosition,
    },
    RuntimeError {
        position: FilePosition,
        error: String,
    },
    InvalidIterator {
        position: FilePosition,
    },
}
enum ScriptControlFlow {
    Value(Variant),
    Return(Variant),
    Break(Variant),
    Err(ScriptError),
}

impl FromResidual for ScriptControlFlow {
    fn from_residual(residual: <Self as Try>::Residual) -> Self {
        residual
    }
}
impl Try for ScriptControlFlow {
    type Output = Variant;
    type Residual = ScriptControlFlow;
    fn from_output(output: Self::Output) -> Self {
        ScriptControlFlow::Value(output)
    }
    fn branch(self) -> ControlFlow<Self::Residual, Self::Output> {
        match self {
            ScriptControlFlow::Value(value) => ControlFlow::Continue(value),
            residual => ControlFlow::Break(residual),
        }
    }
}
impl ScriptError {
    pub fn runtime(text: &str, position: FilePosition) -> Self {
        ScriptError::RuntimeError {
            error: text.to_string(),
            position,
        }
    }
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
        if let Some(variable) = self.variables.lock().get_mut(name) {
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
        position: &FilePosition,
    ) -> Result<(), ScriptError> {
        if let Some(variable) = self.variables.lock().get_mut(name.as_ref()) {
            *variable = value;
        } else {
            return if let Some(previous) = &self.previous {
                previous.set_variable(name, value, position)
            } else {
                Err(ScriptError::VariableNotDefined {
                    variable: name.to_string(),
                    position: position.clone(),
                })
            };
        }

        Ok(())
    }
    pub fn set_variable_top(&self, name: ImmutableString, value: Variant) {
        self.variables.lock().insert(name, value);
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
        parent_stack: Option<&ScopeStack>,
        parameters: Vec<Variant>,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        let stack = ScopeStack::new();
        let mut stack = parent_stack.unwrap_or(&stack);
        for (name, global) in &environment.globals {
            stack.set_variable_top(name.clone(), global.clone());
        }
        if parameters.len() != self.parameter_names.len() {
            return Err(ScriptError::MismatchedParameterCount {
                function_name: self.name.to_string(),
                expected: self
                    .parameter_names
                    .iter()
                    .map(|string| string.to_string())
                    .collect(),
                got: parameters
                    .into_iter()
                    .map(|variant| (*variant.0).type_name())
                    .collect(),
                position: FilePosition::INVALID,
            });
        }
        for (value, name) in parameters.into_iter().zip(self.parameter_names.iter()) {
            stack.set_variable_top(name.clone(), value);
        }
        match Function::execute_block(&mut stack, &self.body, environment) {
            ScriptControlFlow::Value(val) => Ok(val),
            ScriptControlFlow::Return(val) => Ok(val),
            ScriptControlFlow::Break(_) => Err(BreakOutsideLoop),
            ScriptControlFlow::Err(error) => Err(error),
        }
    }
    fn execute_block(
        stack: &ScopeStack,
        block: &StatementBlock,
        environment: &ExecutionEnvironment,
    ) -> ScriptControlFlow {
        let stack = stack.push();
        for statement in &block.statements {
            match statement {
                Statement::Assign {
                    is_let,
                    operator,
                    left,
                    value,
                } => {
                    let value = match Function::eval_expression(&stack, value, environment) {
                        Ok(val) => val,
                        Err(error) => return ScriptControlFlow::Err(error),
                    };
                    let value = match operator {
                        Some(operator) => {
                            let first = match left {
                                Expression::MemberAccess {
                                    expression,
                                    name,
                                    position,
                                } => {
                                    let left = match Function::eval_expression(
                                        &stack,
                                        expression,
                                        environment,
                                    ) {
                                        Ok(val) => val,
                                        Err(error) => return ScriptControlFlow::Err(error),
                                    };
                                    match environment.access_member(&left, name) {
                                        Some(value) => value,
                                        None => {
                                            return ScriptControlFlow::Err(MemberNotFound {
                                                member: name.to_string(),
                                                position: position.clone(),
                                            })
                                        }
                                    }
                                }
                                Expression::ScopedVariable { name, position } => {
                                    match stack.get_variable(name.as_ref()) {
                                        Some(value) => value,
                                        None => {
                                            return ScriptControlFlow::Err(MemberNotFound {
                                                member: name.to_string(),
                                                position: position.clone(),
                                            })
                                        }
                                    }
                                }
                                _ => panic!(),
                            };
                            let operator = format!("operator{operator}");
                            let operator_call = match environment
                                .access_member(&first, &operator.clone().into())
                                .ok_or(ScriptError::MemberNotFound {
                                    member: operator,
                                    position: left.get_file_position().clone(),
                                }) {
                                Ok(call) => call,
                                Err(error) => return ScriptControlFlow::Err(error),
                            };
                            match operator_call.call(
                                vec![value],
                                environment,
                                left.get_file_position(),
                            ) {
                                Ok(value) => value,
                                Err(error) => return ScriptControlFlow::Err(error),
                            }
                        }
                        None => value,
                    };
                    match left {
                        Expression::MemberAccess {
                            expression,
                            name,
                            position,
                        } => {
                            let left =
                                match Function::eval_expression(&stack, expression, environment) {
                                    Ok(val) => val,
                                    Err(error) => return ScriptControlFlow::Err(error),
                                };
                            environment.assign_member(&left, name, &value);
                        }
                        Expression::ScopedVariable { name, position } => {
                            if *is_let {
                                stack.set_variable_top(name.clone(), value);
                            } else {
                                if let Err(error) =
                                    stack.set_variable(name.clone(), value, position)
                                {
                                    return ScriptControlFlow::Err(error);
                                }
                            }
                        }
                        _ => panic!(),
                    }
                }
                Statement::Eval { expression } => {
                    match Function::eval_expression(&stack, expression, environment) {
                        Ok(val) => {}
                        Err(error) => return ScriptControlFlow::Err(error),
                    }
                }
                Statement::If {
                    condition,
                    satisfied,
                    unsatisfied,
                } => {
                    let expression = match Function::eval_expression(&stack, condition, environment)
                    {
                        Ok(val) => val,
                        Err(error) => return ScriptControlFlow::Err(error),
                    };
                    let sat = match bool::from_variant(&expression) {
                        Some(bool) => *bool,
                        None => (*expression.0).type_id() != TypeId::of::<()>(),
                    };
                    let statement = if sat {
                        Some(satisfied)
                    } else {
                        unsatisfied.as_ref()
                    };

                    if let Some(statement) = statement {
                        Function::execute_block(&stack, statement, environment)?;
                    }
                }
                Statement::For {
                    expression,
                    name,
                    body,
                } => {
                    let position = expression.get_file_position();
                    let expression =
                        match Function::eval_expression(&stack, expression, environment) {
                            Ok(val) => val,
                            Err(error) => return ScriptControlFlow::Err(error),
                        };
                    let stack = stack.push();
                    let array = match SharedArray::from_variant(&expression) {
                        Some(array) => array.lock().clone(),
                        None => match Range::<i64>::from_variant(&expression) {
                            Some(range) => range
                                .clone()
                                .into_iter()
                                .map(|i| i.into_variant())
                                .collect(),
                            None => {
                                return ScriptControlFlow::Err(InvalidIterator {
                                    position: position.clone(),
                                });
                            }
                        },
                    };
                    for value in array {
                        stack.set_variable_top(name.clone(), value);
                        match Function::execute_block(&stack, body, environment) {
                            ScriptControlFlow::Break(_) => {
                                break;
                            }
                            other => other,
                        }?;
                    }
                }
                Statement::Return { expression } => {
                    return ScriptControlFlow::Return(if let Some(expression) = expression {
                        match Function::eval_expression(&stack, expression, environment) {
                            Ok(value) => value,
                            Err(error) => return ScriptControlFlow::Err(error),
                        }
                    } else {
                        Variant::NULL()
                    });
                }
                Statement::Break { expression } => {
                    return ScriptControlFlow::Break(if let Some(expression) = expression {
                        match Function::eval_expression(&stack, expression, environment) {
                            Ok(value) => value,
                            Err(error) => return ScriptControlFlow::Err(error),
                        }
                    } else {
                        Variant::NULL()
                    });
                }
            };
        }
        ScriptControlFlow::Value(Variant::NULL())
    }
    fn eval_expression(
        stack: &ScopeStack,
        expression: &Expression,
        environment: &ExecutionEnvironment,
    ) -> ScriptResult {
        match expression {
            Expression::StringLiteral { literal, .. } => Ok(literal.clone().into_variant()),
            Expression::IntLiteral { literal, .. } => Ok((*literal).into_variant()),
            Expression::FloatLiteral { literal, .. } => Ok((*literal).into_variant()),
            Expression::FunctionLiteral { function, .. } => Ok(FunctionVariant {
                function: FunctionType::ScriptFunction(function.clone()),
                this: Variant::NULL(),
            }
            .into_variant()),
            Expression::RangeLiteral {
                start,
                end,
                inclusive,
                ..
            } => Ok((*start..(*end + if *inclusive { 1 } else { 0 })).into_variant()),
            Expression::ScopedVariable { name, position } => {
                let variable = stack
                    .get_variable(name.as_ref())
                    .or_else(|| environment.globals.get(name).cloned())
                    .ok_or(ScriptError::VariableNotDefined {
                        position: position.clone(),
                        variable: name.to_string(),
                    });
                variable
            }
            Expression::Call {
                expression,
                parameters,
                position,
            } => {
                let expression = Function::eval_expression(stack, expression, environment)?;
                let parameters = parameters
                    .iter()
                    .map(|parameter| Function::eval_expression(stack, parameter, environment))
                    .collect::<Result<Vec<_>, ScriptError>>()?;
                Ok(expression.call(parameters, environment, position)?)
            }
            Expression::MemberAccess {
                expression,
                name,
                position,
            } => {
                let value = Function::eval_expression(stack, expression, environment)?;
                environment
                    .access_member(&value, name)
                    .ok_or(ScriptError::MemberNotFound {
                        member: name.to_string(),
                        position: position.clone(),
                    })
            }
            Expression::Operator {
                first,
                second,
                operator,
                position,
            } => {
                let first = Function::eval_expression(stack, first, environment)?;
                let second = Function::eval_expression(stack, second, environment)?;
                if operator.as_ref() == "!=" {
                    let operator_call = environment
                        .access_member(&first, &"operator==".into())
                        .ok_or(ScriptError::MemberNotFound {
                            member: "operator==".to_string(),
                            position: position.clone(),
                        })?;
                    Ok((!*(bool::from_variant_error(
                        &operator_call.call(vec![second], environment, position)?,
                        position,
                    )?))
                    .into_variant())
                } else {
                    let operator = format!("operator{operator}");
                    let operator_call = environment
                        .access_member(&first, &operator.clone().into())
                        .ok_or(ScriptError::MemberNotFound {
                            member: operator,
                            position: position.clone(),
                        })?;
                    Ok(operator_call.call(vec![second], environment, position)?)
                }
            }
            Expression::UnaryOperator {
                expression,
                operator,
                position,
            } => {
                let expression = Function::eval_expression(stack, expression, environment)?;
                let operator = format!("uoperator{operator}");
                let operator_call = environment
                    .access_member(&expression, &operator.clone().into())
                    .ok_or(ScriptError::MemberNotFound {
                        member: operator,
                        position: position.clone(),
                    })?;
                Ok(operator_call.call(vec![], environment, position)?)
            }
        }
    }
}
impl Debug for Function {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fn {}({}){{{:?}}}",
            self.name,
            self.parameter_names.join(","),
            self.body
        )
    }
}
pub struct ExecutionEnvironment {
    types: HashMap<TypeId, TypeInfo>,
    globals: HashMap<ImmutableString, Variant>,
    custom_names: Arc<Mutex<HashMap<TypeId, ImmutableString>>>,
}
impl ExecutionEnvironment {
    pub fn new() -> Self {
        ExecutionEnvironment {
            types: HashMap::new(),
            globals: HashMap::new(),
            custom_names: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    fn access_member(&self, value: &Variant, name: &ImmutableString) -> Option<Variant> {
        self.types
            .get(&((*value.0).type_id()))?
            .access_member(value, name)
    }
    fn assign_member(&self, left: &Variant, name: &ImmutableString, value: &Variant) {
        self.types
            .get(&((*left.0).type_id()))
            .unwrap()
            .assign_member(left, name, value)
    }
    pub fn register_member<
        T: Primitive,
        N: Into<ImmutableString>,
        F: Fn(&T) -> Option<R> + Send + Sync + 'static,
        R: IntoVariant,
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
                Box::new(move |this| {
                    function(T::from_variant(this).unwrap()).map(|r| r.into_variant())
                }),
            );
    }
    pub fn register_method<
        T: Primitive,
        F: IntoScriptMethod<T, A> + Send + Sync + 'static,
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
    pub fn register_global<N: Into<ImmutableString>>(&mut self, name: N, value: Variant) {
        self.globals.insert(name.into(), value);
    }
    pub fn get_global(&self, name: &ImmutableString) -> Option<&Variant> {
        self.globals.get(name)
    }
    pub fn register_function<F: IntoScriptFunction<A>, N: Into<ImmutableString>, A: 'static>(
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
    pub fn register_custom_name<T: Primitive, N: Into<ImmutableString>>(&mut self, custom_name: N) {
        self.custom_names
            .lock()
            .insert(TypeId::of::<T>(), custom_name.into());
    }
    pub fn register_default_accessor<
        T: Primitive,
        F: Fn(&Variant, ImmutableString) -> Option<Variant> + Send + Sync + 'static,
    >(
        &mut self,
        function: F,
    ) {
        self.types
            .entry(TypeId::of::<T>())
            .or_insert(TypeInfo::new())
            .default = Some(Box::new(function));
    }
    pub fn register_setter<
        T: Primitive,
        F: Fn(&Variant, ImmutableString, &Variant) + Send + Sync + 'static,
    >(
        &mut self,
        function: F,
    ) {
        self.types
            .entry(TypeId::of::<T>())
            .or_insert(TypeInfo::new())
            .setter = Some(Box::new(function));
    }
    pub fn get_type_info(&self, type_id: TypeId) -> Option<&TypeInfo> {
        self.types.get(&type_id)
    }
    pub fn get_type_name_resolver(&self) -> TypeNameResolver {
        TypeNameResolver(self.custom_names.clone())
    }
}
pub struct TypeNameResolver(pub Arc<Mutex<HashMap<TypeId, ImmutableString>>>);
pub struct TypeInfo {
    members: HashMap<ImmutableString, Box<dyn Fn(&Variant) -> Option<Variant> + Send + Sync>>,
    default: Option<Box<dyn Fn(&Variant, ImmutableString) -> Option<Variant> + Send + Sync>>,
    setter: Option<Box<dyn Fn(&Variant, ImmutableString, &Variant) + Send + Sync>>,
}
impl TypeInfo {
    pub fn new() -> Self {
        TypeInfo {
            members: HashMap::new(),
            default: None,
            setter: None,
        }
    }
    pub fn assign_member(&self, this: &Variant, name: &ImmutableString, value: &Variant) {
        if let Some(setter) = &self.setter {
            setter(this, name.clone(), value);
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
            if let Some(function) = FunctionVariant::from_variant(variant) {
                return Some(
                    FunctionVariant {
                        this: value.clone(),
                        function: function.function.clone(),
                    }
                    .into_variant(),
                );
            }
        }
        variant
    }
}

trait IntoScriptResult {
    fn into_script_result(self) -> ScriptResult;
}
impl<T: Primitive> IntoScriptResult for Result<T, ScriptError> {
    fn into_script_result(self) -> ScriptResult {
        if TypeId::of::<T>() == TypeId::of::<Variant>() {
            self.map(|ok| unsafe { std::mem::transmute_copy(&std::mem::ManuallyDrop::new(ok)) })
        } else {
            self.map(|ok| ok.into_variant())
        }
    }
}

pub trait IntoScriptFunction<A: 'static> {
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync>;
}
pub trait IntoScriptMethod<S: Primitive, A: 'static> {
    fn into_method(self) -> Box<dyn Fn(&S, Vec<Variant>) -> ScriptResult + Send + Sync>;
}
impl IntoScriptFunction<()> for Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        self
    }
}
impl<S: Primitive> IntoScriptMethod<S, ()>
    for Box<dyn Fn(&S, Vec<Variant>) -> ScriptResult + Send + Sync>
{
    fn into_method(self) -> Box<dyn Fn(&S, Vec<Variant>) -> ScriptResult + Send + Sync> {
        self
    }
}
macro_rules! register_into_function {
    ($($i:tt,)*) => {
        impl<T, R, $($i,)*> IntoScriptFunction<($($i,)*)> for T
        where
            $($i: Primitive,)*
            R: IntoScriptResult,
            T: Fn($(&$i,)*) -> R + Send + Sync + 'static,
        {
            fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
                Box::new(move |mut args| {
                    /*if args.len() != 1 {
                        return Err(ScriptError::MismatchedParameterCount);
                    }*/
                    //todo
                    self($($i::from_variant_error(&args.remove(0), &FilePosition::INVALID)?,)*).into_script_result()
                })
            }
        }

    };
}
macro_rules! register_into_method {
    ($($i:tt,)*) => {
        impl<T, R, A,$($i,)*> IntoScriptMethod<A, (A,$($i,)*)> for T
        where
            A: Primitive,
            $($i: Primitive,)*
            R: IntoScriptResult,
            T: Fn(&A,$(&$i,)*) -> R + Send + Sync + 'static,
        {
            fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
                Box::new(move |this, mut args| {
                    /*if args.len() != 0 {
                        return Err(ScriptError::MismatchedParameterCount);
                    }*/
                    //todo
                    self(this,$($i::from_variant_error(&args.remove(0), &FilePosition::INVALID)?,)*).into_script_result()
                })
            }
        }};
}
impl<T, R> IntoScriptFunction<()> for T
where
    R: IntoScriptResult,
    T: Fn() -> R + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 0 {
                return Err(ScriptError::MismatchedParameterCount {
                    function_name: "rust_defined".to_string(),
                    got: Vec::new(),
                    expected: Vec::new(),
                    position: FilePosition::INVALID,
                });
            }
            self().into_script_result()
        })
    }
}
register_into_function!(A,);
register_into_function!(A, B,);
register_into_function!(A, B, C,);
register_into_function!(A, B, C, D,);
register_into_function!(A, B, C, D, E,);
register_into_function!(A, B, C, D, E, F,);
register_into_function!(A, B, C, D, E, F, G,);
register_into_function!(A, B, C, D, E, F, G, H,);
register_into_method!();
register_into_method!(B,);
register_into_method!(B, C,);
register_into_method!(B, C, D,);
register_into_method!(B, C, D, E,);
register_into_method!(B, C, D, E, F,);
register_into_method!(B, C, D, E, F, G,);
register_into_method!(B, C, D, E, F, G, H,);
/*impl<T, R, A> IntoScriptFunction<(A,)> for T
where
    A: Primitive,
    R: IntoScriptResult,
    T: Fn(&A) -> R + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 1 {
                return Err(ScriptError::MismatchedParameterCount);
            }
            self(A::from_variant_error(&args[0])?).into_script_result()
        })
    }
}
impl<T, R, A> IntoScriptMethod<A, (A,)> for T
where
    A: Primitive,
    R: IntoScriptResult,
    T: Fn(&A) -> R + Send + Sync + 'static,
{
    fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |this, args| {
            if args.len() != 0 {
                return Err(ScriptError::MismatchedParameterCount);
            }
            self(this).into_script_result()
        })
    }
}
impl<T, R, A, B> IntoScriptFunction<(A, B)> for T
where
    A: Primitive,
    B: Primitive,
    R: IntoScriptResult,
    T: Fn(&A, &B) -> R + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 2 {
                return Err(ScriptError::MismatchedParameterCount);
            }
            self(
                A::from_variant_error(&args[0])?,
                B::from_variant_error(&args[1])?,
            )
            .into_script_result()
        })
    }
}
impl<T, R, A, B> IntoScriptMethod<A, (A, B)> for T
where
    A: Primitive,
    B: Primitive,
    R: IntoScriptResult,
    T: Fn(&A, &B) -> R + Send + Sync + 'static,
{
    fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |this, args| {
            if args.len() != 1 {
                return Err(ScriptError::MismatchedParameterCount);
            }
            self(this, B::from_variant_error(&args[0])?).into_script_result()
        })
    }
}
impl<T, R, A, B, C> IntoScriptFunction<(A, B, C)> for T
where
    A: Primitive,
    B: Primitive,
    C: Primitive,
    R: IntoScriptResult,
    T: Fn(&A, &B, &C) -> R + Send + Sync + 'static,
{
    fn into_function(self) -> Box<dyn Fn(Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |args| {
            if args.len() != 3 {
                return Err(ScriptError::MismatchedParameterCount);
            }
            self(
                A::from_variant_error(&args[0])?,
                B::from_variant_error(&args[1])?,
                C::from_variant_error(&args[2])?,
            )
            .into_script_result()
        })
    }
}
impl<T, R, A, B, C> IntoScriptMethod<A, (A, B, C)> for T
where
    A: Primitive,
    B: Primitive,
    C: Primitive,
    R: IntoScriptResult,
    T: Fn(&A, &B, &C) -> R + Send + Sync + 'static,
{
    fn into_method(self) -> Box<dyn Fn(&A, Vec<Variant>) -> ScriptResult + Send + Sync> {
        Box::new(move |this, args| {
            if args.len() != 2 {
                return Err(ScriptError::MismatchedParameterCount);
            }
            self(
                this,
                B::from_variant_error(&args[0])?,
                C::from_variant_error(&args[1])?,
            )
            .into_script_result()
        })
    }
}
*/
