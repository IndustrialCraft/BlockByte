use crate::eval::{ExecutionEnvironment, ScriptError};
use crate::variant::{Array, FromVariant, IntoVariant, Map, Variant};
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
    environment.register_function("print", |text: &ImmutableString| {
        println!("{}", text);
        Ok(())
    });
}
