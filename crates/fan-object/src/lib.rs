//! 凡语言对象系统。
//!
//! 包含值的表示（`Object`）、作用域（`Environment`）、运行实例（`Runtime`）
//! 与运行时实体（函数、模型、实例、接口、命名类型、内建函数）。
//!
//! ```text
//! fan-ast → fan-object → fan-eval → fan-native → fan-cli
//! ```
//!
//! ## 与 Go 版的一处架构差异
//!
//! Go 版把函数/模型/实例放在 `internal/evaluator`，靠 `object.Object` 接口实现
//! 跨包扩展。Rust 的 `enum` 无法跨 crate 增补变体，若照搬会产生
//! `fan-object ↔ fan-eval` 循环依赖。因此本 crate 把二者统一收纳，
//! `Object` 可直接持有运行时实体。
//!
//! ## 值语义与引用语义
//!
//! 凡语言规范（§9.5）规定数组、字典、模型实例为引用语义，其余为值语义。
//! 本 crate 用 `Rc<RefCell<…>>` 承载引用语义类型，
//! `Clone` 即“浅拷贝=共享”，无需额外的两套 API。

pub mod collection;
pub mod environment;
pub mod interface;
pub mod kind;
pub mod object;
pub mod runtime;
pub mod runtime_entity;
pub mod tag_context;

pub use collection::{
    dict_key, error_message, is_error, new_error, new_error_with_code, new_tuple, valid_dict_key,
    wrap_error, DictInner, ErrorValue, FanArray, FanDict, ModuleValue, StackFrame, TupleValue,
};
pub use environment::{
    check_decl_type, check_single_decl_type, coerce_decl_type, format_decl_type, Binding,
    Environment,
};
pub use interface::{interface_missing_method, interface_sig_conflict};
pub use kind::{kind_display_name, primitive_kind_from_name, runtime_type_name, Kind};
pub use object::{Named, Object};
pub use runtime::{is_builtin_type_name, kind_type_name, FnFrame, Output, Runtime};
pub use runtime_entity::{
    Builtin, Class, Function, Instance, Interface, NamedType, NativeGroup, NativeImpl, NativeSpec,
};
pub use tag_context::TagContextValue;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 对象系统可独立构造与判等() {
        let a = Object::array(vec![Object::int(1), Object::string("甲")]);
        let b = Object::array(vec![Object::int(1), Object::string("甲")]);
        assert_eq!(a, b);
        assert_eq!(a.inspect(), "[1, 甲]");
    }

    #[test]
    fn 环境与运行实例协同工作() {
        let mut env = Environment::new();
        env.declare(
            "甲",
            Object::int(1),
            false,
            fan_ast::DeclType::any(),
            fan_ast::Visibility::File,
        )
        .unwrap();
        assert_eq!(env.get("甲"), Some(Object::int(1)));
        assert_eq!(env.runtime().borrow().stack_depth(), 0);
    }

    #[test]
    fn 数组共享修改对所有持有者可见() {
        let a = FanArray::new(vec![Object::int(1)]);
        let env_holder = Object::Array(a.clone());
        a.push(Object::int(2));
        // 通过原始句柄修改，对象视图同步变化
        assert_eq!(env_holder.inspect(), "[1, 2]");
    }
}
