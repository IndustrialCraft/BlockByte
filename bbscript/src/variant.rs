use crate::eval::{ExecutionEnvironment, Function, ScriptError, ScriptResult};
use dyn_clone::DynClone;
use std::any::Any;
use std::sync::Arc;

pub trait Primitive: Any + DynClone + Send + Sync {
    #[must_use]
    fn as_any(&self) -> &dyn Any;
}
dyn_clone::clone_trait_object!(Primitive);

impl<T: Any + Clone + Send + Sync> Primitive for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct Variant(Box<dyn Primitive>);

impl Variant {
    pub fn call(&self, args: Vec<Variant>, environment: &ExecutionEnvironment) -> ScriptResult {
        match FunctionVariant::from_variant(self) {
            Some(function_variant) => match &function_variant.function {
                FunctionType::ScriptFunction(function) => {
                    function.run(function_variant.this.clone(), args, environment)
                }
                FunctionType::RustFunction(function) => {
                    function(function_variant.this.clone(), args)
                }
            },
            None => Err(ScriptError::NonFunctionCalled),
        }
    }
    #[allow(non_snake_case)]
    pub fn NULL() -> Variant {
        Variant(Box::new(()))
    }
}

pub trait IntoVariant {
    fn into_variant(self) -> Variant;
}
impl<T: Primitive> IntoVariant for T {
    fn into_variant(self) -> Variant {
        Variant(Box::new(self))
    }
}

pub trait FromVariant {
    fn from_variant(variant: &Variant) -> Option<&Self>;
}
impl<T: Primitive> FromVariant for T {
    fn from_variant(variant: &Variant) -> Option<&Self> {
        (&*variant.0).as_any().downcast_ref()
    }
}

#[derive(Clone)]
pub struct FunctionVariant {
    pub this: Variant,
    pub function: FunctionType,
}
#[derive(Clone)]
pub enum FunctionType {
    ScriptFunction(Arc<Function>),
    RustFunction(Arc<dyn Fn(Variant, Vec<Variant>) -> ScriptResult + Send + Sync>),
}
