use crate::eval::{Function, ScriptResult};
use dyn_clone::DynClone;
use dyn_eq::DynEq;
use immutable_string::ImmutableString;
use std::any::{Any, TypeId};
use std::cmp::Ordering;
use std::sync::Arc;

pub trait Primitive: Any + DynClone + DynEq {}

dyn_clone::clone_trait_object!(Primitive);
dyn_eq::eq_trait_object!(Primitive);

#[derive(Clone)]
pub enum Variant {
    Null,
    Shared(Arc<dyn Any>),
    Primitive(Box<dyn Primitive>),
    Function(Box<Variant>, FunctionType),
}
impl Eq for Variant {}
#[derive(Clone)]
pub enum FunctionType {
    ScriptFunction(Arc<Function>),
    RustFunction(Arc<dyn Fn(Variant, Vec<Variant>) -> ScriptResult>),
}
impl Eq for FunctionType {}
impl PartialEq for FunctionType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FunctionType::ScriptFunction(first), FunctionType::ScriptFunction(second)) => {
                Arc::ptr_eq(first, second)
            }
            (FunctionType::RustFunction(first), FunctionType::RustFunction(second)) => {
                Arc::ptr_eq(first, second)
            }
            _ => false,
        }
    }
}
/*impl Clone for Variant {
    fn clone(&self) -> Self {
        match self {
            Variant::Null => Variant::Null,
            Variant::Shared(shared) => Variant::Shared(shared.clone()),
            Variant::Primitive(value) => Variant::Primitive(value.clone()),
            Variant::Function(this, function) => Variant::Function(this.clone(), function.clone()),
        }
    }
}*/
impl PartialEq for Variant {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Variant::Null, Variant::Null) => true,
            (Variant::Shared(first), Variant::Shared(second)) => Arc::ptr_eq(first, second),
            (Variant::Primitive(first), Variant::Primitive(second)) => first == second,
            (_, _) => false,
        }
    }
}
impl Variant {
    pub fn get_ref(&self) -> &dyn Any {
        match self {
            Variant::Shared(value) => value.as_ref(),
            Variant::Primitive(value) => value.as_ref(),
            Variant::Null | Variant::Function(_, _) => &(),
        }
    }
}
impl Primitive for i64 {}
impl Primitive for bool {}
impl Primitive for ImmutableString {}

#[derive(Clone)]
pub struct Sf64(pub f64);
impl PartialEq for Sf64 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Sf64 {}
impl Primitive for Sf64 {}
