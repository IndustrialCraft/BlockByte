use crate::eval::{Function, ScriptError, ScriptResult};
use dyn_clone::DynClone;
use dyn_eq::DynEq;
use immutable_string::ImmutableString;
use std::any::{Any, TypeId};
use std::cmp::Ordering;
use std::sync::Arc;

pub trait Primitive: Any + DynClone + Send + Sync {
    #[must_use]
    fn as_any(&self) -> &dyn Any;
}
dyn_clone::clone_trait_object!(Primitive);

#[derive(Clone)]
pub enum Variant {
    Null,
    Shared(Arc<dyn Any>),
    Primitive(Box<dyn Primitive>),
    Function(Box<Variant>, FunctionType),
}
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
impl Variant {
    pub fn get_ref(&self) -> &dyn Any {
        match self {
            Variant::Shared(value) => value.as_ref(),
            Variant::Primitive(value) => value.as_ref(),
            Variant::Null | Variant::Function(_, _) => &(),
        }
    }
}
impl<T: Any + Clone + Send + Sync> Primitive for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
pub fn convert_variant_list_1<A: 'static>(args: &[Variant]) -> Result<(&A,), ScriptError> {
    if args.len() != 1 {
        return Err(ScriptError::TypeError);
    }
    Ok((args[0]
        .get_ref()
        .downcast_ref()
        .ok_or(ScriptError::TypeError)?,))
}
pub fn convert_variant_list_2<A: 'static, B: 'static>(
    args: &[Variant],
) -> Result<(&A, &B), ScriptError> {
    if args.len() != 2 {
        return Err(ScriptError::TypeError);
    }
    Ok((
        args[0]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[1]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
    ))
}
pub fn convert_variant_list_3<A: 'static, B: 'static, C: 'static>(
    args: &[Variant],
) -> Result<(&A, &B, &C), ScriptError> {
    if args.len() != 3 {
        return Err(ScriptError::TypeError);
    }
    Ok((
        args[0]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[1]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[2]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
    ))
}
pub fn convert_variant_list_4<A: 'static, B: 'static, C: 'static, D: 'static>(
    args: &[Variant],
) -> Result<(&A, &B, &C, &D), ScriptError> {
    if args.len() != 4 {
        return Err(ScriptError::TypeError);
    }
    Ok((
        args[0]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[1]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[2]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[3]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
    ))
}
pub fn convert_variant_list_5<A: 'static, B: 'static, C: 'static, D: 'static, E: 'static>(
    args: &[Variant],
) -> Result<(&A, &B, &C, &D, &E), ScriptError> {
    if args.len() != 5 {
        return Err(ScriptError::TypeError);
    }
    Ok((
        args[0]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[1]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[2]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[3]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
        args[4]
            .get_ref()
            .downcast_ref()
            .ok_or(ScriptError::TypeError)?,
    ))
}
