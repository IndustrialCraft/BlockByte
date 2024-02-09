use crate::eval::{ExecutionEnvironment, ScriptResult};
use crate::variant::IntoVariant;
use immutable_string::ImmutableString;

pub fn register_defaults(environment: &mut ExecutionEnvironment) {
    macro_rules! register_operators {
        ($operator_type:ty) => {
            register_operators!($operator_type, +);
            register_operators!($operator_type, -);
            register_operators!($operator_type, *);
            register_operators!($operator_type, /);
        };
        ($operator_type:ty,$operator:tt) => {
            environment.register_method(format!("operator{}", stringify!($operator)), move |first: &$operator_type, second: &$operator_type| {
                Ok((*first $operator *second).into_variant())
            });
        }
    }
    macro_rules! register_comparison {
        ($operator_type:ty,false) => {
            environment.register_method(
                "operator==",
                move |first: &$operator_type, second: &$operator_type| {
                    Ok((*first == *second).into_variant())
                },
            );
            environment.register_method(
                "operator!=",
                move |first: &$operator_type, second: &$operator_type| {
                    Ok((*first != *second).into_variant())
                },
            );
        };
        ($operator_type:ty, true) => {
            register_comparison!($operator_type, false);
            environment.register_method(
                "operator>",
                move |first: &$operator_type, second: &$operator_type| {
                    Ok((*first > *second).into_variant())
                },
            );
            environment.register_method(
                "operator<",
                move |first: &$operator_type, second: &$operator_type| {
                    Ok((*first < *second).into_variant())
                },
            );
            environment.register_method(
                "operator>=",
                move |first: &$operator_type, second: &$operator_type| {
                    Ok((*first >= *second).into_variant())
                },
            );
            environment.register_method(
                "operator<=",
                move |first: &$operator_type, second: &$operator_type| {
                    Ok((*first <= *second).into_variant())
                },
            );
        };
    }

    register_operators!(i64);
    register_operators!(f64);

    register_comparison!(i64, true);
    register_comparison!(f64, true);
    register_comparison!(bool, false);
    register_comparison!(ImmutableString, false);
}
