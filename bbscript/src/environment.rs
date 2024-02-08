use crate::eval::{ExecutionEnvironment, ScriptResult};
use crate::variant::IntoVariant;

pub fn register_defaults(environment: &mut ExecutionEnvironment) {
    environment.register_method("operator+", move |first: &i64, second: &i64| {
        Ok((*first + *second).into_variant())
    });
}
