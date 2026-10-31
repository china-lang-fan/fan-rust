//! 运算符语义。
//!
//! 全部规则与 Go 版 `evaluator` 中的 `evalAdd`/`evalEq`/`evalCompare` 等一一对应：
//! 数值族（整数/字节）优先走整数运算，否则提升为小数；
//! 字符串、字符、数组各有专门的拼接规则。

use crate::error::FanError;
use fan_ast::Position;
use fan_object::{Kind, Object};

/// 把整数族（整数、字节）转为 `i64`。命名类型会先解包。
pub fn int_value(o: &Object) -> Option<i64> {
    match o.unwrap_named() {
        Object::Int(v) => Some(v),
        Object::Byte(v) => Some(v as i64),
        _ => None,
    }
}

/// 数值提升为小数（整数、小数、字节）。非数值返回 `None`。
pub fn to_float(o: &Object) -> Option<f64> {
    match o.unwrap_named() {
        Object::Int(v) => Some(v as f64),
        Object::Float(v) => Some(v),
        Object::Byte(v) => Some(v as f64),
        _ => None,
    }
}

/// 是否为整数族（整数/字节）。
pub fn is_int_family(o: &Object) -> bool {
    matches!(o.unwrap_named(), Object::Int(_) | Object::Byte(_))
}

/// 码点偏移后的字符；超出 Unicode 范围返回 `None`。
fn shift_char(c: char, delta: i64) -> Option<char> {
    let code = c as i64 + delta;
    if !(0..=0x10FFFF).contains(&code) {
        return None;
    }
    // 代理区（U+D800–U+DFFF）不是合法标量值。
    if (0xD800..=0xDFFF).contains(&code) {
        return None;
    }
    char::from_u32(code as u32)
}

// ---------------------------------------------------------------- 算术

/// 加法：数组拼接、字符串拼接、字符与字符串、字符与整数偏移，最后才是数值相加。
pub fn add(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let l = left.unwrap_named();
    let r = right.unwrap_named();

    // 数组 + 数组 => 拼接
    if let (Object::Array(a), Object::Array(b)) = (&l, &r) {
        let mut out = a.elements();
        out.extend(b.elements());
        return Ok(Object::Array(fan_object::FanArray::new(out)));
    }
    // 字符串 + 字符串
    if let (Object::Str(a), Object::Str(b)) = (&l, &r) {
        return Ok(Object::string(format!("{}{}", a, b)));
    }
    // 字符串 + 字符 / 字符 + 字符串
    if let (Object::Str(a), Object::Char(b)) = (&l, &r) {
        return Ok(Object::string(format!("{}{}", a, b)));
    }
    if let (Object::Char(a), Object::Str(b)) = (&l, &r) {
        return Ok(Object::string(format!("{}{}", a, b)));
    }
    // 字符 + 整数族 => 字符
    if let Object::Char(c) = &l {
        if let Some(rv) = int_value(&r) {
            return shift_char(*c, rv)
                .map(Object::Char)
                .ok_or_else(|| FanError::at("字符偏移超出 Unicode 范围", pos));
        }
    }
    if let Object::Char(c) = &r {
        if let Some(lv) = int_value(&l) {
            return shift_char(*c, lv)
                .map(Object::Char)
                .ok_or_else(|| FanError::at("字符偏移超出 Unicode 范围", pos));
        }
    }
    // 整数族 + 整数族 => 整数
    if let (Some(lv), Some(rv)) = (int_value(&l), int_value(&r)) {
        return Ok(Object::Int(lv.wrapping_add(rv)));
    }
    match (to_float(&l), to_float(&r)) {
        (Some(lv), Some(rv)) => Ok(Object::Float(lv + rv)),
        _ => Err(FanError::at(
            format!("加法不支持 {} + {}", l.kind(), r.kind()),
            pos,
        )),
    }
}

pub fn sub(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let (l, r) = (left.unwrap_named(), right.unwrap_named());
    if let (Some(lv), Some(rv)) = (int_value(&l), int_value(&r)) {
        return Ok(Object::Int(lv.wrapping_sub(rv)));
    }
    match (to_float(&l), to_float(&r)) {
        (Some(lv), Some(rv)) => Ok(Object::Float(lv - rv)),
        _ => Err(FanError::at(
            format!("减法不支持 {} - {}", l.kind(), r.kind()),
            pos,
        )),
    }
}

pub fn mul(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let (l, r) = (left.unwrap_named(), right.unwrap_named());
    if let (Some(lv), Some(rv)) = (int_value(&l), int_value(&r)) {
        return Ok(Object::Int(lv.wrapping_mul(rv)));
    }
    match (to_float(&l), to_float(&r)) {
        (Some(lv), Some(rv)) => Ok(Object::Float(lv * rv)),
        _ => Err(FanError::at(
            format!("乘法不支持 {} * {}", l.kind(), r.kind()),
            pos,
        )),
    }
}

pub fn div(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let (l, r) = (left.unwrap_named(), right.unwrap_named());
    // 整数除法：Go 版对 0 报错，对负数采用向零取整（与 Go 的 / 一致）
    if let (Some(lv), Some(rv)) = (int_value(&l), int_value(&r)) {
        if rv == 0 {
            return Err(FanError::at("除数不能为零", pos));
        }
        return Ok(Object::Int(lv.wrapping_div(rv)));
    }
    match (to_float(&l), to_float(&r)) {
        (Some(lv), Some(rv)) => {
            if rv == 0.0 {
                return Err(FanError::at("除数不能为零", pos));
            }
            Ok(Object::Float(lv / rv))
        }
        _ => Err(FanError::at(
            format!("除法不支持 {} / {}", l.kind(), r.kind()),
            pos,
        )),
    }
}

/// 取余仅支持整数族（与 Go 版一致）。
pub fn modulus(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let (l, r) = (left.unwrap_named(), right.unwrap_named());
    match (int_value(&l), int_value(&r)) {
        (Some(lv), Some(rv)) => {
            if rv == 0 {
                return Err(FanError::at("除数不能为零", pos));
            }
            Ok(Object::Int(lv.wrapping_rem(rv)))
        }
        _ => Err(FanError::at("取余仅支持整数 % 整数", pos)),
    }
}

/// 位运算：位与、位或、异或、左移、右移。仅支持整数族。
pub fn bitwise(pos: Position, left: &Object, right: &Object, op: &str) -> Result<Object, FanError> {
    let (l, r) = (left.unwrap_named(), right.unwrap_named());
    let (lv, rv) = match (int_value(&l), int_value(&r)) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(FanError::at(
                format!(
                    "{}仅支持整数家族（整数/字节），实际 {} 与 {}",
                    op,
                    l.kind(),
                    r.kind()
                ),
                pos,
            ))
        }
    };
    let result = match op {
        "位与" => lv & rv,
        "位或" => lv | rv,
        "异或" => lv ^ rv,
        "左移" => {
            if rv < 0 {
                return Err(FanError::at("左移位数不能为负", pos));
            }
            if rv > 63 {
                0
            } else {
                lv.wrapping_shl(rv as u32)
            }
        }
        "右移" => {
            if rv < 0 {
                return Err(FanError::at("右移位数不能为负", pos));
            }
            if rv > 63 {
                0
            } else {
                // Go 的 >> 是算术右移，保留符号位
                lv.wrapping_shr(rv as u32)
            }
        }
        other => return Err(FanError::at(format!("未知位运算符：{}", other), pos)),
    };
    Ok(Object::Int(result))
}

// ---------------------------------------------------------------- 比较

/// 相等比较。命名类型先解包；整数族与字符可按码点比较。
pub fn eq(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let l = left.unwrap_named();
    let r = right.unwrap_named();

    // 空值只与空值相等。与任意非空值比较都应得到「不相等」，而不是报错——
    // 标准库大量使用 `X != 空` 判断可选值是否存在。
    if matches!(l.kind(), Kind::Nil) || matches!(r.kind(), Kind::Nil) {
        return Ok(Object::Bool(
            matches!(l.kind(), Kind::Nil) && matches!(r.kind(), Kind::Nil),
        ));
    }

    if let Some(lv) = int_value(&l) {
        if let Some(rv) = int_value(&r) {
            return Ok(Object::Bool(lv == rv));
        }
        if let Object::Char(c) = &r {
            return Ok(Object::Bool(lv == *c as i64));
        }
        return Err(FanError::at(
            format!("不同类型不可比较：{} 与 {}", l.kind(), r.kind()),
            pos,
        ));
    }
    if let Object::Char(c) = &l {
        if let Some(rv) = int_value(&r) {
            return Ok(Object::Bool(*c as i64 == rv));
        }
    }

    if l.kind() != r.kind() {
        return Err(FanError::at(
            format!("不同类型不可比较：{} 与 {}", l.kind(), r.kind()),
            pos,
        ));
    }
    match (&l, &r) {
        (Object::Int(a), Object::Int(b)) => Ok(Object::Bool(a == b)),
        (Object::Float(a), Object::Float(b)) => Ok(Object::Bool(a == b)),
        (Object::Str(a), Object::Str(b)) => Ok(Object::Bool(a == b)),
        (Object::Bool(a), Object::Bool(b)) => Ok(Object::Bool(a == b)),
        (Object::Byte(a), Object::Byte(b)) => Ok(Object::Bool(a == b)),
        (Object::Char(a), Object::Char(b)) => Ok(Object::Bool(a == b)),
        (Object::Nil, Object::Nil) => Ok(Object::Bool(true)),
        (Object::Array(a), Object::Array(b)) => Ok(Object::Bool(a == b)),
        (Object::Dict(a), Object::Dict(b)) => Ok(Object::Bool(a == b)),
        (Object::Error(a), Object::Error(b)) => {
            Ok(Object::Bool(a.borrow().message == b.borrow().message))
        }
        _ => Err(FanError::at(format!("不支持 {} 比较", l.kind()), pos)),
    }
}

/// 大小比较。`accept` 为可接受的比较结果：`-1` 小于、`0` 等于、`1` 大于。
pub fn compare(
    pos: Position,
    left: &Object,
    right: &Object,
    accept: &[i8],
) -> Result<Object, FanError> {
    let l = left.unwrap_named();
    let r = right.unwrap_named();

    if matches!(l.kind(), Kind::Array) || matches!(r.kind(), Kind::Array) {
        return Err(FanError::at("数组不支持大小比较", pos));
    }
    // 字符之间按码点比较
    if let (Object::Char(a), Object::Char(b)) = (&l, &r) {
        let cmp = match a.cmp(b) {
            std::cmp::Ordering::Less => -1i8,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        return Ok(Object::Bool(accept.contains(&cmp)));
    }
    match (to_float(&l), to_float(&r)) {
        (Some(lv), Some(rv)) => {
            let cmp = if lv < rv {
                -1i8
            } else if lv > rv {
                1
            } else {
                0
            };
            Ok(Object::Bool(accept.contains(&cmp)))
        }
        _ => Err(FanError::at(
            format!("比较不支持 {} 与 {}", l.kind(), r.kind()),
            pos,
        )),
    }
}

/// 逻辑与（两侧都必须为布尔）。
pub fn logical_and(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let l = left.unwrap_named();
    let r = right.unwrap_named();
    match &l {
        Object::Bool(false) => return Ok(Object::Bool(false)),
        Object::Bool(true) => {}
        other => {
            return Err(FanError::at(
                format!("且 左侧必须是布尔，实际是 {}", other.kind()),
                pos,
            ))
        }
    }
    match &r {
        Object::Bool(_) => Ok(r.clone()),
        other => Err(FanError::at(
            format!("且 右侧必须是布尔，实际是 {}", other.kind()),
            pos,
        )),
    }
}

/// 逻辑或（两侧都必须为布尔）。
pub fn logical_or(pos: Position, left: &Object, right: &Object) -> Result<Object, FanError> {
    let l = left.unwrap_named();
    let r = right.unwrap_named();
    match &l {
        Object::Bool(true) => return Ok(Object::Bool(true)),
        Object::Bool(false) => {}
        other => {
            return Err(FanError::at(
                format!("或 左侧必须是布尔，实际是 {}", other.kind()),
                pos,
            ))
        }
    }
    match &r {
        Object::Bool(_) => Ok(r.clone()),
        other => Err(FanError::at(
            format!("或 右侧必须是布尔，实际是 {}", other.kind()),
            pos,
        )),
    }
}

// ---------------------------------------------------------------- 一元

/// 一元运算：负号、位取反、非。
pub fn unary(pos: Position, op: &str, right: &Object) -> Result<Object, FanError> {
    let r = right.unwrap_named();
    match op {
        "-" => match &r {
            Object::Int(v) => Ok(Object::Int(v.wrapping_neg())),
            Object::Float(v) => Ok(Object::Float(-v)),
            other => Err(FanError::at(format!("负号不支持 {}", other.kind()), pos)),
        },
        "位取反" | "~" => match int_value(&r) {
            Some(n) => Ok(Object::Int(!n)),
            None => Err(FanError::at(
                format!("位取反仅支持整数家族，实际是 {}", r.kind()),
                pos,
            )),
        },
        "非" | "!" => match &r {
            Object::Bool(v) => Ok(Object::Bool(!v)),
            other => Err(FanError::at(
                format!("非 必须是布尔，实际是 {}", other.kind()),
                pos,
            )),
        },
        other => Err(FanError::at(format!("未知一元运算符：{}", other), pos)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Position = Position::START;

    fn i(v: i64) -> Object {
        Object::Int(v)
    }

    fn f(v: f64) -> Object {
        Object::Float(v)
    }

    #[test]
    fn 整数族提取与数值提升() {
        assert_eq!(int_value(&i(5)), Some(5));
        assert_eq!(int_value(&Object::Byte(5)), Some(5));
        assert_eq!(int_value(&f(5.0)), None);
        assert_eq!(to_float(&i(5)), Some(5.0));
        assert_eq!(to_float(&f(5.5)), Some(5.5));
        assert_eq!(to_float(&Object::Str(std::rc::Rc::from("x"))), None);
        assert!(is_int_family(&i(1)));
        assert!(!is_int_family(&f(1.0)));
    }

    #[test]
    fn 命名类型参与运算时解包() {
        let n = Object::named("状态", i(3));
        assert_eq!(int_value(&n), Some(3));
        assert_eq!(add(P, &n, &i(4)).unwrap(), i(7));
    }

    #[test]
    fn 整数加减乘() {
        assert_eq!(add(P, &i(1), &i(2)).unwrap(), i(3));
        assert_eq!(sub(P, &i(5), &i(2)).unwrap(), i(3));
        assert_eq!(mul(P, &i(3), &i(4)).unwrap(), i(12));
    }

    #[test]
    fn 整数除法向零取整且除零报错() {
        assert_eq!(div(P, &i(7), &i(2)).unwrap(), i(3));
        assert_eq!(div(P, &i(-7), &i(2)).unwrap(), i(-3));
        assert_eq!(div(P, &i(7), &i(0)).unwrap_err().message, "除数不能为零");
    }

    #[test]
    fn 整数与小数混合提升为小数() {
        assert_eq!(add(P, &i(1), &f(2.5)).unwrap(), f(3.5));
        assert_eq!(sub(P, &i(1), &f(0.5)).unwrap(), f(0.5));
        assert_eq!(mul(P, &i(2), &f(1.5)).unwrap(), f(3.0));
        assert_eq!(div(P, &i(1), &f(4.0)).unwrap(), f(0.25));
    }

    #[test]
    fn 小数除零报错() {
        assert_eq!(
            div(P, &f(1.0), &f(0.0)).unwrap_err().message,
            "除数不能为零"
        );
    }

    #[test]
    fn 取余仅支持整数() {
        assert_eq!(modulus(P, &i(7), &i(3)).unwrap(), i(1));
        assert_eq!(
            modulus(P, &i(7), &i(0)).unwrap_err().message,
            "除数不能为零"
        );
        assert_eq!(
            modulus(P, &f(7.0), &f(3.0)).unwrap_err().message,
            "取余仅支持整数 % 整数"
        );
    }

    #[test]
    fn 字符串拼接() {
        assert_eq!(
            add(P, &Object::string("你好"), &Object::string("世界")).unwrap(),
            Object::string("你好世界")
        );
    }

    #[test]
    fn 字符串与字符拼接() {
        assert_eq!(
            add(P, &Object::string("你"), &Object::Char('好')).unwrap(),
            Object::string("你好")
        );
        assert_eq!(
            add(P, &Object::Char('你'), &Object::string("好")).unwrap(),
            Object::string("你好")
        );
    }

    #[test]
    fn 字符加整数得到偏移字符() {
        assert_eq!(
            add(P, &Object::Char('A'), &i(1)).unwrap(),
            Object::Char('B')
        );
        assert_eq!(
            add(P, &i(1), &Object::Char('A')).unwrap(),
            Object::Char('B')
        );
    }

    #[test]
    fn 字符偏移越界报错() {
        assert!(add(P, &Object::Char('\u{10FFFF}'), &i(1)).is_err());
    }

    #[test]
    fn 数组拼接生成新数组() {
        let a = Object::array(vec![i(1)]);
        let b = Object::array(vec![i(2)]);
        let out = add(P, &a, &b).unwrap();
        assert_eq!(out.inspect(), "[1, 2]");
        // 原数组不受影响
        assert_eq!(a.inspect(), "[1]");
    }

    #[test]
    fn 加法不支持的组合报错() {
        let err = add(P, &i(1), &Object::Str(std::rc::Rc::from("x"))).unwrap_err();
        assert_eq!(err.message, "加法不支持 整数 + 字符串");
    }

    #[test]
    fn 位运算() {
        assert_eq!(
            bitwise(P, &i(0b1100), &i(0b1010), "位与").unwrap(),
            i(0b1000)
        );
        assert_eq!(
            bitwise(P, &i(0b1100), &i(0b1010), "位或").unwrap(),
            i(0b1110)
        );
        assert_eq!(
            bitwise(P, &i(0b1100), &i(0b1010), "异或").unwrap(),
            i(0b0110)
        );
        assert_eq!(bitwise(P, &i(1), &i(3), "左移").unwrap(), i(8));
        assert_eq!(bitwise(P, &i(8), &i(2), "右移").unwrap(), i(2));
    }

    #[test]
    fn 位移位数为负报错() {
        assert_eq!(
            bitwise(P, &i(1), &i(-1), "左移").unwrap_err().message,
            "左移位数不能为负"
        );
        assert_eq!(
            bitwise(P, &i(1), &i(-1), "右移").unwrap_err().message,
            "右移位数不能为负"
        );
    }

    #[test]
    fn 位移位数过大结果为零() {
        assert_eq!(bitwise(P, &i(1), &i(64), "左移").unwrap(), i(0));
        assert_eq!(bitwise(P, &i(1), &i(64), "右移").unwrap(), i(0));
    }

    #[test]
    fn 位运算仅支持整数族() {
        let err = bitwise(P, &f(1.0), &i(1), "位与").unwrap_err();
        assert!(err.message.starts_with("位与仅支持整数家族"));
    }

    #[test]
    fn 相等比较基础类型() {
        assert_eq!(eq(P, &i(1), &i(1)).unwrap(), Object::Bool(true));
        assert_eq!(eq(P, &i(1), &i(2)).unwrap(), Object::Bool(false));
        assert_eq!(eq(P, &f(1.5), &f(1.5)).unwrap(), Object::Bool(true));
        assert_eq!(
            eq(P, &Object::string("甲"), &Object::string("甲")).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &Object::Bool(true), &Object::Bool(true)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &Object::Byte(1), &Object::Byte(1)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &Object::Char('中'), &Object::Char('中')).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &Object::nil(), &Object::nil()).unwrap(),
            Object::Bool(true)
        );
    }

    #[test]
    fn 整数与字节与字符可交叉比较() {
        assert_eq!(
            eq(P, &i(65), &Object::Byte(65)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &i(65), &Object::Char('A')).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &Object::Char('A'), &i(65)).unwrap(),
            Object::Bool(true)
        );
    }

    #[test]
    fn 错误与空值不相等() {
        let e = fan_object::new_error("失败");
        assert_eq!(eq(P, &e, &Object::nil()).unwrap(), Object::Bool(false));
        assert_eq!(eq(P, &Object::nil(), &e).unwrap(), Object::Bool(false));
    }

    #[test]
    fn 不同类型比较报错() {
        let err = eq(P, &i(1), &Object::Str(std::rc::Rc::from("x"))).unwrap_err();
        assert_eq!(err.message, "不同类型不可比较：整数 与 字符串");
    }

    #[test]
    fn 数组与字典按内容比较() {
        assert_eq!(
            eq(P, &Object::array(vec![i(1)]), &Object::array(vec![i(1)])).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            eq(P, &Object::array(vec![i(1)]), &Object::array(vec![i(2)])).unwrap(),
            Object::Bool(false)
        );
        let mut d1 = fan_object::FanDict::new();
        d1.set(i(1), i(10));
        let mut d2 = fan_object::FanDict::new();
        d2.set(i(1), i(10));
        assert_eq!(
            eq(P, &Object::Dict(d1), &Object::Dict(d2)).unwrap(),
            Object::Bool(true)
        );
    }

    #[test]
    fn 错误比较按消息() {
        let a = fan_object::new_error("失败");
        let b = fan_object::new_error("失败");
        let c = fan_object::new_error("其他");
        assert_eq!(eq(P, &a, &b).unwrap(), Object::Bool(true));
        assert_eq!(eq(P, &a, &c).unwrap(), Object::Bool(false));
    }

    #[test]
    fn 大小比较数值() {
        assert_eq!(compare(P, &i(1), &i(2), &[-1]).unwrap(), Object::Bool(true));
        assert_eq!(compare(P, &i(2), &i(2), &[0]).unwrap(), Object::Bool(true));
        assert_eq!(compare(P, &i(3), &i(2), &[1]).unwrap(), Object::Bool(true));
        // 小于等于同时接受 -1 与 0
        assert_eq!(
            compare(P, &i(2), &i(2), &[0, -1]).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            compare(P, &i(3), &i(2), &[0, -1]).unwrap(),
            Object::Bool(false)
        );
    }

    #[test]
    fn 字符按码点比较() {
        assert_eq!(
            compare(P, &Object::Char('A'), &Object::Char('B'), &[-1]).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            compare(P, &Object::Char('B'), &Object::Char('A'), &[1]).unwrap(),
            Object::Bool(true)
        );
    }

    #[test]
    fn 数组不支持大小比较() {
        let err = compare(P, &Object::array(vec![]), &i(1), &[-1]).unwrap_err();
        assert_eq!(err.message, "数组不支持大小比较");
    }

    #[test]
    fn 逻辑与短路且要求布尔() {
        assert_eq!(
            logical_and(P, &Object::Bool(false), &Object::Bool(true)).unwrap(),
            Object::Bool(false)
        );
        // 左侧为假时右侧不求值，右侧非法类型也不报错
        assert_eq!(
            logical_and(P, &Object::Bool(false), &i(1)).unwrap(),
            Object::Bool(false)
        );
        assert_eq!(
            logical_and(P, &Object::Bool(true), &Object::Bool(true)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            logical_and(P, &i(1), &Object::Bool(true))
                .unwrap_err()
                .message,
            "且 左侧必须是布尔，实际是 整数"
        );
        assert_eq!(
            logical_and(P, &Object::Bool(true), &i(1))
                .unwrap_err()
                .message,
            "且 右侧必须是布尔，实际是 整数"
        );
    }

    #[test]
    fn 逻辑或短路且要求布尔() {
        assert_eq!(
            logical_or(P, &Object::Bool(true), &Object::Bool(false)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            logical_or(P, &Object::Bool(true), &i(1)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            logical_or(P, &Object::Bool(false), &Object::Bool(false)).unwrap(),
            Object::Bool(false)
        );
        assert_eq!(
            logical_or(P, &i(1), &Object::Bool(true))
                .unwrap_err()
                .message,
            "或 左侧必须是布尔，实际是 整数"
        );
    }

    #[test]
    fn 一元负号() {
        assert_eq!(unary(P, "-", &i(5)).unwrap(), i(-5));
        assert_eq!(unary(P, "-", &f(5.5)).unwrap(), f(-5.5));
        assert_eq!(
            unary(P, "-", &Object::Str(std::rc::Rc::from("x")))
                .unwrap_err()
                .message,
            "负号不支持 字符串"
        );
    }

    #[test]
    fn 位取反与非() {
        assert_eq!(unary(P, "位取反", &i(0)).unwrap(), i(-1));
        assert_eq!(
            unary(P, "非", &Object::Bool(true)).unwrap(),
            Object::Bool(false)
        );
        assert_eq!(
            unary(P, "!", &Object::Bool(false)).unwrap(),
            Object::Bool(true)
        );
        assert_eq!(
            unary(P, "非", &i(1)).unwrap_err().message,
            "非 必须是布尔，实际是 整数"
        );
    }

    #[test]
    fn 未知一元运算符报错() {
        assert!(unary(P, "?", &i(1))
            .unwrap_err()
            .message
            .starts_with("未知一元运算符"));
    }
}
