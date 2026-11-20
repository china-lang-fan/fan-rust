//! 字节码运算语义（M1 子集）。
//!
//! 为保持 fan-vm 不反向依赖 fan-eval，这里独立实现常用算术/比较/逻辑运算，
//! 语义与树遍历求值器对齐：整数优先、小数提升、字符串/数组拼接、结构相等。
//! 更复杂的位运算与混合类型规则在后续里程碑补全，最终与 fan-eval 的运算符
//! 表完全对齐并纳入双引擎对拍。

use fan_object::Object;

use crate::error::{VmError, VmResult};

fn integer(value: &Object) -> Option<i64> {
    match value {
        Object::Int(i) => Some(*i),
        Object::Byte(b) => Some(*b as i64),
        Object::Named(n) => integer(&n.value),
        _ => None,
    }
}

fn float(value: &Object) -> Option<f64> {
    match value {
        Object::Float(f) => Some(*f),
        Object::Int(i) => Some(*i as f64),
        Object::Byte(b) => Some(*b as f64),
        Object::Named(n) => float(&n.value),
        _ => None,
    }
}

pub fn apply_binary(op: &str, left: Object, right: Object) -> VmResult<Object> {
    match op {
        "+" | "加" => add(left, right),
        "-" | "减" => arithmetic(op, left, right),
        "*" | "乘" => arithmetic(op, left, right),
        "/" | "除" => arithmetic(op, left, right),
        "%" | "模" => arithmetic(op, left, right),
        "==" | "等于" => Ok(Object::Bool(objects_equal(&left, &right))),
        "!=" | "不等于" => Ok(Object::Bool(!objects_equal(&left, &right))),
        "<" | "小于" | ">" | "大于" | "<=" | "小于等于" | ">=" | "大于等于" => {
            compare(op, left, right)
        }
        "且" | "&&" => Ok(if is_truthy(&left) { right } else { left }),
        "或" | "||" => Ok(if is_truthy(&left) { left } else { right }),
        "位与" | "&" | "位或" | "|" | "异或" | "^" | "左移" | "<<" | "右移" | ">>" => {
            bitwise(op, left, right)
        }
        other => Err(VmError::new(format!("VM 暂不支持二元运算符：{other}"))),
    }
}

/// 位运算：仅支持整数族（整数/字节/命名整数），语义对齐树遍历求值器。
///
/// 右移采用 `wrapping_shr`，对有符号 i64 即算术右移（保留符号位），
/// 与 Go 的 `>>` 及 fan-eval 一致；移位量为负报错、超过 63 结果为 0。
fn bitwise(op: &str, left: Object, right: Object) -> VmResult<Object> {
    let lv = integer(&left).ok_or_else(|| {
        VmError::new(format!(
            "{op}仅支持整数家族（整数/字节），实际 {} 与 {}",
            left.kind(),
            right.kind()
        ))
    })?;
    let rv = integer(&right).ok_or_else(|| {
        VmError::new(format!(
            "{op}仅支持整数家族（整数/字节），实际 {} 与 {}",
            left.kind(),
            right.kind()
        ))
    })?;
    let result = match op {
        "位与" | "&" => lv & rv,
        "位或" | "|" => lv | rv,
        "异或" | "^" => lv ^ rv,
        "左移" | "<<" => {
            if rv < 0 {
                return Err(VmError::new("左移位数不能为负"));
            }
            if rv > 63 {
                0
            } else {
                lv.wrapping_shl(rv as u32)
            }
        }
        "右移" | ">>" => {
            if rv < 0 {
                return Err(VmError::new("右移位数不能为负"));
            }
            if rv > 63 {
                0
            } else {
                lv.wrapping_shr(rv as u32)
            }
        }
        _ => unreachable!("位运算分支已限定"),
    };
    Ok(Object::Int(result))
}

pub fn apply_unary(op: &str, operand: Object) -> VmResult<Object> {
    match op {
        "-" | "负" => match integer(&operand) {
            Some(i) => Ok(Object::Int(-i)),
            None => match float(&operand) {
                Some(f) => Ok(Object::Float(-f)),
                None => Err(VmError::new("一元负号要求整数或小数")),
            },
        },
        "!" | "非" => Ok(Object::Bool(!is_truthy(&operand))),
        "位取反" | "~" => match integer(&operand) {
            Some(i) => Ok(Object::Int(!i)),
            None => Err(VmError::new(format!(
                "位取反仅支持整数家族，实际是 {}",
                operand.kind()
            ))),
        },
        other => Err(VmError::new(format!("VM 暂不支持一元运算符：{other}"))),
    }
}

fn add(left: Object, right: Object) -> VmResult<Object> {
    if let (Some(a), Some(b)) = (integer(&left), integer(&right)) {
        return Ok(Object::Int(a + b));
    }
    if let (Some(a), Some(b)) = (float(&left), float(&right)) {
        return Ok(Object::Float(a + b));
    }
    match (&left, &right) {
        (Object::Str(a), Object::Str(b)) => Ok(Object::string(format!("{a}{b}"))),
        (Object::Array(a), Object::Array(b)) => {
            let mut merged = a.elements().clone();
            merged.extend(b.elements().iter().cloned());
            Ok(Object::array(merged))
        }
        _ => Err(VmError::new(format!(
            "无法对 {} 与 {} 执行加法",
            left.kind(),
            right.kind()
        ))),
    }
}

fn arithmetic(op: &str, left: Object, right: Object) -> VmResult<Object> {
    if let (Some(a), Some(b)) = (integer(&left), integer(&right)) {
        let value = match op {
            "-" | "减" => a - b,
            "*" | "乘" => a * b,
            "/" | "除" => {
                if b == 0 {
                    return Err(VmError::new("整数除零"));
                }
                a / b
            }
            "%" | "模" => {
                if b == 0 {
                    return Err(VmError::new("整数模零"));
                }
                a % b
            }
            _ => unreachable!("算术运算分支已限定"),
        };
        return Ok(Object::Int(value));
    }
    let a = float(&left).ok_or_else(|| VmError::new(format!("{} 不是数值", left.kind())))?;
    let b = float(&right).ok_or_else(|| VmError::new(format!("{} 不是数值", right.kind())))?;
    let value = match op {
        "-" | "减" => a - b,
        "*" | "乘" => a * b,
        "/" | "除" => a / b,
        "%" | "模" => a % b,
        _ => unreachable!("算术运算分支已限定"),
    };
    Ok(Object::Float(value))
}

fn compare(op: &str, left: Object, right: Object) -> VmResult<Object> {
    let ord = if let (Some(a), Some(b)) = (integer(&left), integer(&right)) {
        a.cmp(&b)
    } else if let (Some(a), Some(b)) = (float(&left), float(&right)) {
        a.partial_cmp(&b)
            .ok_or_else(|| VmError::new("小数比较遇到非有限值"))?
    } else if let (Object::Str(a), Object::Str(b)) = (&left, &right) {
        a.as_ref().cmp(b.as_ref())
    } else {
        return Err(VmError::new(format!(
            "无法比较 {} 与 {}",
            left.kind(),
            right.kind()
        )));
    };
    use std::cmp::Ordering::*;
    let result = match op {
        "<" | "小于" => ord == Less,
        ">" | "大于" => ord == Greater,
        "<=" | "小于等于" => ord != Greater,
        ">=" | "大于等于" => ord != Less,
        _ => unreachable!("比较运算分支已限定"),
    };
    Ok(Object::Bool(result))
}

fn objects_equal(left: &Object, right: &Object) -> bool {
    left == right
}

fn is_truthy(value: &Object) -> bool {
    !matches!(value, Object::Nil | Object::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 整数算术与提升() {
        assert_eq!(
            apply_binary("+", Object::int(2), Object::int(3)).unwrap(),
            Object::Int(5)
        );
        assert!(matches!(
            apply_binary("*", Object::int(2), Object::Float(2.5)).unwrap(),
            Object::Float(f) if f == 5.0
        ));
    }

    #[test]
    fn 字符串拼接与比较() {
        let s = apply_binary("加", Object::string("凡"), Object::string("语言")).unwrap();
        assert_eq!(s, Object::string("凡语言"));
        assert_eq!(
            apply_binary("小于", Object::int(1), Object::int(2)).unwrap(),
            Object::Bool(true)
        );
    }

    #[test]
    fn 除零与非数值报错() {
        assert!(apply_binary("/", Object::int(1), Object::int(0)).is_err());
        assert!(apply_unary("负", Object::string("x")).is_err());
    }

    #[test]
    fn 位运算与算术右移() {
        assert_eq!(
            apply_binary("位与", Object::int(0b1100), Object::int(0b1010)).unwrap(),
            Object::Int(0b1000)
        );
        assert_eq!(
            apply_binary("位或", Object::int(0b1100), Object::int(0b1010)).unwrap(),
            Object::Int(0b1110)
        );
        assert_eq!(
            apply_binary("异或", Object::int(0b1100), Object::int(0b1010)).unwrap(),
            Object::Int(0b0110)
        );
        assert_eq!(
            apply_binary("左移", Object::int(1), Object::int(3)).unwrap(),
            Object::Int(8)
        );
        assert_eq!(
            apply_binary("右移", Object::int(8), Object::int(2)).unwrap(),
            Object::Int(2)
        );
        // 负数右移保留符号位（算术右移）。
        assert_eq!(
            apply_binary("右移", Object::int(-8), Object::int(1)).unwrap(),
            Object::Int(-4)
        );
        assert_eq!(
            apply_unary("位取反", Object::int(0)).unwrap(),
            Object::Int(-1)
        );
        assert!(apply_binary("左移", Object::int(1), Object::int(-1)).is_err());
        assert!(apply_binary("位与", Object::int(1), Object::string("x")).is_err());
    }
}
