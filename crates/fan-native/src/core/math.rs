//! 数学组内建：平方根与幂。
//!
//! 对应 `fan-sdk/数学.凡`。该模块是「凡语言优先」的范例：绝对值、取整、
//! 四舍五入、最值、总和、平均值、勾股定理全部用凡语言实现，只有这两个
//! 函数需要宿主——它们依赖浮点指令，凡语言无法自行实现。

use fan_object::{NativeSpec, Object};
use std::rc::Rc;

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "sqrt",
            Some(1),
            Rc::new(|_rt, args| {
                let v = args[0].as_float().ok_or("平方根 需要数值")?;
                if v < 0.0 {
                    return Err("负数不能开平方".to_string());
                }
                Ok(vec![Object::Float(v.sqrt())])
            }),
        ),
        NativeSpec::core(
            "pow",
            Some(2),
            Rc::new(|_rt, args| {
                let base = args[0].as_float().ok_or("幂 的底数需要数值")?;
                let exp = args[1].as_float().ok_or("幂 的指数需要数值")?;
                Ok(vec![Object::Float(base.powf(exp))])
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = fan_object::Runtime::new();
        (spec.func)(&mut rt, args)
    }

    #[test]
    fn 平方根() {
        assert_eq!(
            call("sqrt", vec![Object::Float(9.0)]).unwrap()[0],
            Object::Float(3.0)
        );
        assert_eq!(
            call("sqrt", vec![Object::int(16)]).unwrap()[0],
            Object::Float(4.0)
        );
    }

    #[test]
    fn 负数开平方报错() {
        assert!(call("sqrt", vec![Object::Float(-1.0)])
            .unwrap_err()
            .contains("负数"));
    }

    #[test]
    fn 幂运算() {
        assert_eq!(
            call("pow", vec![Object::Float(2.0), Object::Float(10.0)]).unwrap()[0],
            Object::Float(1024.0)
        );
        assert_eq!(
            call("pow", vec![Object::int(2), Object::int(3)]).unwrap()[0],
            Object::Float(8.0)
        );
        // 勾股定理：3-4-5
        let (a, b) = (3.0f64, 4.0f64);
        let c = (a.powf(2.0) + b.powf(2.0)).sqrt();
        assert_eq!(c, 5.0);
    }

    #[test]
    fn 幂支持负指数与分数指数() {
        assert_eq!(
            call("pow", vec![Object::Float(2.0), Object::Float(-1.0)]).unwrap()[0],
            Object::Float(0.5)
        );
        assert_eq!(
            call("pow", vec![Object::Float(4.0), Object::Float(0.5)]).unwrap()[0],
            Object::Float(2.0)
        );
    }
}
