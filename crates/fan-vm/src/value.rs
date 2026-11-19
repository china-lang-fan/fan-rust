//! 把字节码常量池里的纯数据 [`ConstValue`] 装箱为运行时 [`Object`]。
//!
//! 这是 fan-code（编译期纯数据）与 fan-object（运行时值）之间唯一的接触点，
//! 保证 fan-code 本身无需依赖 fan-object。

use fan_code::ConstValue;
use fan_object::Object;

pub fn const_to_object(c: &ConstValue) -> Object {
    match c {
        ConstValue::Nil => Object::Nil,
        ConstValue::Bool(b) => Object::Bool(*b),
        ConstValue::Int(i) => Object::Int(*i),
        ConstValue::Float(f) => Object::Float(*f),
        ConstValue::Byte(b) => Object::Byte(*b),
        ConstValue::Char(ch) => Object::Char(*ch),
        ConstValue::Str(s) => Object::string(s.clone()),
    }
}
