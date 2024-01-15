use crate::eval::{Function, ScriptResult};
use immutable_string::ImmutableString;
use std::any::{Any, TypeId};
use std::sync::Arc;

#[derive(Clone)]
pub enum Variant {
    Null,
    Shared(Arc<dyn Any>),
    String(ImmutableString),
    Int(i64),
    Float(f64),
    Bool(bool),
    Function(Box<Variant>, FunctionType),
}
#[derive(Clone)]
pub enum FunctionType {
    ScriptFunction(Arc<Function>),
    RustFunction(Arc<dyn Fn(Variant, Vec<Variant>) -> ScriptResult>),
}
impl PartialEq for Variant {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Variant::Null, Variant::Null) => true,
            (Variant::Shared(first), Variant::Shared(second)) => Arc::ptr_eq(first, second),
            (Variant::String(first), Variant::String(second)) => first == second,
            (Variant::Int(first), Variant::Int(second)) => first == second,
            (Variant::Float(first), Variant::Float(second)) => first == second,
            (Variant::Bool(first), Variant::Bool(second)) => first == second,
            (_, _) => false,
        }
    }
}
impl Variant {
    pub fn get_ref(&self) -> &dyn Any {
        match self {
            Variant::Shared(value) => &**value,
            Variant::String(value) => value,
            Variant::Int(value) => value,
            Variant::Float(value) => value,
            Variant::Bool(value) => value,
            Variant::Null | Variant::Function(_, _) => &(),
        }
    }
    pub fn get_type(&self) -> TypeId {
        match self {
            Variant::Shared(value) => value.as_ref().type_id(),
            Variant::String(_) => TypeId::of::<ImmutableString>(),
            Variant::Int(_) => TypeId::of::<i64>(),
            Variant::Float(_) => TypeId::of::<f64>(),
            Variant::Bool(_) => TypeId::of::<bool>(),
            Variant::Null | Variant::Function(_, _) => TypeId::of::<()>(),
        }
    }
}
