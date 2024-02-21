use crate::eval::{
    ExecutionEnvironment, Function, ScopeStack, ScriptError, ScriptResult, TypeNameResolver,
};
use crate::lex::FilePosition;
use dyn_clone::DynClone;
use immutable_string::ImmutableString;
use parking_lot::Mutex;
use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

#[derive(Debug)]
pub struct TypeName(TypeId, &'static str);
impl TypeName {
    pub fn new<T: 'static>() -> TypeName {
        TypeName(TypeId::of::<T>(), type_name::<T>())
    }
    pub fn resolve_name(&self, resolver: &TypeNameResolver) -> ImmutableString {
        resolver
            .0
            .lock()
            .get(&self.0)
            .cloned()
            .unwrap_or(self.1.into())
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
pub struct Variant(pub Box<dyn Primitive>);

impl Debug for Variant {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Variant({})", (*self.0).type_name().1)
    }
}

impl Variant {
    pub fn from_str(text: &str) -> Variant {
        ImmutableString::from(text).into_variant()
    }
    pub fn from_option<T: IntoVariant>(option: Option<T>) -> Variant {
        option.map(|e| e.into_variant()).unwrap_or(Variant::NULL())
    }
    pub fn into_option<'a, T: FromVariant>(
        variant: &'a Variant,
        position: &FilePosition,
    ) -> Result<Option<&'a T>, ScriptError> {
        if (*variant.0).as_any().type_id() == TypeId::of::<()>() {
            return Ok(None);
        }
        T::from_variant_error(variant, position).map(|variant| Some(variant))
    }
    pub fn call(
        &self,
        args: Vec<Variant>,
        environment: &ExecutionEnvironment,
        position: &FilePosition,
    ) -> ScriptResult {
        match FunctionVariant::from_variant(self) {
            Some(function_variant) => match &function_variant.function {
                FunctionType::ScriptFunction(function) => {
                    let scope = ScopeStack::new();
                    scope.set_variable_top("this".into(), function_variant.this.clone());
                    function.run(Some(&scope), args, environment)
                }
                FunctionType::RustFunction(function) => {
                    function(function_variant.this.clone(), args)
                }
            },
            None => Err(ScriptError::NonFunctionCalled {
                position: position.clone(),
            }),
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
        if self.type_id() == TypeId::of::<Variant>() {
            unsafe { std::mem::transmute_copy(&std::mem::ManuallyDrop::new(self)) }
        } else {
            Variant(Box::new(self))
        }
    }
}

pub trait FromVariant {
    fn from_variant(variant: &Variant) -> Option<&Self>;
    fn from_variant_error<'a>(
        variant: &'a Variant,
        position: &FilePosition,
    ) -> Result<&'a Self, ScriptError>;
}
impl<T: Primitive> FromVariant for T {
    fn from_variant(variant: &Variant) -> Option<&Self> {
        if TypeId::of::<T>() == TypeId::of::<Variant>() {
            let address = variant as *const Variant;
            return Some(unsafe { &*address.cast() });
        }
        (&*variant.0).as_any().downcast_ref()
    }
    fn from_variant_error<'a>(
        variant: &'a Variant,
        position: &FilePosition,
    ) -> Result<&'a Self, ScriptError> {
        Self::from_variant(variant).ok_or(ScriptError::MismatchedType {
            expected: TypeName::new::<T>(),
            got: (*variant.0).type_name(),
            position: position.clone(),
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
impl FromIterator<Variant> for Array {
    fn from_iter<T: IntoIterator<Item = Variant>>(iter: T) -> Self {
        Arc::new(Mutex::new(Vec::from_iter(iter)))
    }
}
pub type Map = Arc<Mutex<HashMap<ImmutableString, Variant>>>;
/*impl FromIterator<(ImmutableString, Variant)> for Map {
    fn from_iter<T: IntoIterator<Item = (ImmutableString, Variant)>>(iter: T) -> Self {
        Arc::new(Mutex::new(HashMap::from_iter(iter)))
    }
}*/
