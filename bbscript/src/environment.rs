use crate::eval::{ExecutionEnvironment, ScriptError};
use crate::lex::FilePosition;
use crate::variant::{Array, FromVariant, IntoVariant, Map, Primitive, Variant};
use immutable_string::ImmutableString;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub fn register_defaults(environment: &mut ExecutionEnvironment) {
    environment.register_global("null", Variant::NULL());
    environment.register_global("false", false.into_variant());
    environment.register_global("true", true.into_variant());

    let type_name_resolver = environment.get_type_name_resolver();
    environment.register_function("type_of", move |variant: &Variant| {
        Ok((*variant.0).type_name().resolve_name(&type_name_resolver))
    });

    environment.register_function("is_null", |variant: &Variant| {
        Ok((*variant.0).as_any().type_id() == TypeId::of::<()>())
    });

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
    macro_rules! register_to_string {
        ($operator_type:ty) => {
            environment.register_method("to_string", move |this: &$operator_type| {
                Ok(ImmutableString::from(this.to_string()))
            });
        };
    }
    environment.register_custom_name::<ImmutableString, _>("String");
    environment.register_custom_name::<Map, _>("Map");
    environment.register_custom_name::<Array, _>("Array");

    register_to_string!(i64);
    register_to_string!(f64);
    register_to_string!(bool);

    register_operators!(i64);
    register_operators!(f64);

    register_comparison!(i64, true);
    register_comparison!(f64, true);
    register_comparison!(bool, false);
    register_comparison!(ImmutableString, false);

    environment.register_method(
        "operator+",
        |this: &ImmutableString, other: &ImmutableString| {
            Ok(ImmutableString::from(format!("{}{}", this, other)))
        },
    );

    environment.register_method(
        "operator%",
        |first: &i64, second: &i64| Ok(*first % *second),
    );

    environment.register_method("uoperator!", |this: &bool| Ok(!*this));
    environment.register_method("uoperator-", |this: &i64| Ok(-*this));
    environment.register_method("uoperator-", |this: &f64| Ok(-*this));

    environment.register_function("Array", || Ok(Arc::new(Mutex::new(Vec::<Variant>::new()))));
    environment.register_method("get", |this: &Array, index: &i64| {
        let this = this.lock();
        this.get(*index as usize)
            .cloned()
            .ok_or(ScriptError::RuntimeError {
                error: format!(
                    "array index {} out of bounds, length: {}",
                    *index,
                    this.len()
                ),
                position: FilePosition::INVALID,
            })
    });
    environment.register_method("set", |this: &Array, index: &i64, value: &Variant| {
        let mut this = this.lock();
        if *index > this.len() as i64 {
            return Err(ScriptError::RuntimeError {
                error: format!(
                    "array index {} out of bounds, length: {}",
                    *index,
                    this.len()
                ),
                position: FilePosition::INVALID,
            });
        }
        this.insert(*index as usize, value.clone());
        Ok(Variant::NULL())
    });
    environment.register_method("push", |this: &Array, value: &Variant| {
        let mut this = this.lock();
        this.push(value.clone());
        Ok(Variant::NULL())
    });
    environment.register_function("Map", || {
        Ok(Arc::new(Mutex::new(
            HashMap::<ImmutableString, Variant>::new(),
        )))
    });
    environment.register_method("get", |this: &Map, key: &ImmutableString| {
        Ok(Variant::from_option(this.lock().get(key).cloned()))
    });
    environment.register_method(
        "set",
        |this: &Map, key: &ImmutableString, value: &Variant| {
            Ok(this.lock().insert(key.clone(), value.clone()))
        },
    );
    environment.register_default_accessor::<Map, _>(|this: &Variant, key: ImmutableString| {
        let map = Map::from_variant(this)?;
        map.lock().get(&key).cloned()
    });
    environment.register_setter::<Map, _>(
        |this: &Variant, key: ImmutableString, value: &Variant| {
            if let Some(map) = Map::from_variant(this) {
                map.lock().insert(key, value.clone());
            }
        },
    );
    environment.register_function("print", |text: &ImmutableString| {
        println!("{}", text);
        Ok(())
    });
    environment.register_function("min", |n1: &i64, n2: &i64| Ok(*n1.min(n2)));
    environment.register_function("max", |n1: &i64, n2: &i64| Ok(*n1.max(n2)));
    environment.register_function("clamp", |value: &i64, min: &i64, max: &i64| {
        Ok(*value.max(min).min(max))
    });
}
