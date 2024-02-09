use crate::eval::{ExecutionEnvironment, Function, ScriptError, ScriptResult};
use dyn_clone::DynClone;
use immutable_string::ImmutableString;
use parking_lot::Mutex;
use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct TypeName(TypeId, &'static str);
impl TypeName {
    pub fn new<T: 'static>() -> TypeName {
        TypeName(TypeId::of::<T>(), type_name::<T>())
    }
    pub fn resolve_name<'a>(&'a self, env: &'a ExecutionEnvironment) -> &str {
        env.get_type_info(self.0)
            .and_then(|info| info.custom_name.as_ref().map(|name| name.as_ref()))
            .unwrap_or(self.1)
    }
}

pub trait Primitive: Any + DynClone + Send + Sync {
    #[must_use]
    fn as_any(&self) -> &dyn Any;
    fn type_name(&self) -> TypeName;
}
dyn_clone::clone_trait_object!(Primitive);

impl<T: Any + Clone + Send + Sync> Primitive for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn type_name(&self) -> TypeName {
        TypeName::new::<T>()
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
    fn from_variant_error(variant: &Variant) -> Result<&Self, ScriptError>;
}
impl<T: Primitive> FromVariant for T {
    fn from_variant(variant: &Variant) -> Option<&Self> {
        if TypeId::of::<T>() == TypeId::of::<Variant>() {
            let address = variant as *const Variant;
            return Some(unsafe { &*address.cast() });
        }
        (&*variant.0).as_any().downcast_ref()
    }
    fn from_variant_error(variant: &Variant) -> Result<&Self, ScriptError> {
        Self::from_variant(variant).ok_or(ScriptError::MismatchedType {
            expected: TypeName::new::<T>(),
            got: variant.type_name(),
        })
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

pub type Array = Arc<Mutex<Vec<Variant>>>;
pub type Map = Arc<Mutex<HashMap<ImmutableString, Variant>>>;