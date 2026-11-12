//! 系统组内建：输出、类型反射、错误构造、长度与追加。
//!
//! 对应 `fan-sdk/系统.凡`。这 9 个能力凡语言无法自行表达：
//! 输出需要宿主的输出通道，类型名需要运行时反射，错误对象需要携带调用栈。

use fan_object::{ErrorValue, NativeSpec, Object};
use std::rc::Rc;

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "print",
            None,
            Rc::new(|rt, args| {
                rt.print_objects(&args);
                Ok(vec![])
            }),
        ),
        // 取运行时类型名，返回英文标识（"integer"/"string"/"array"…），
        // 凡语言层据此分派，见 `转换.凡` 的 `判断 系统.类型名(值)`。
        NativeSpec::core(
            "type",
            Some(1),
            Rc::new(|_rt, args| {
                // 自定义类型报它自己的名字：类型名(状态(1)) 是「状态」，
                // 即便底层是整数——这才是调用方区分类型身份的依据
                let name = match &args[0] {
                    Object::Named(n) => n.type_name.clone(),
                    other => fan_object::runtime_type_name(other).to_string(),
                };
                Ok(vec![Object::string(name)])
            }),
        ),
        NativeSpec::core(
            "len",
            Some(1),
            Rc::new(|_rt, args| {
                let n = match &args[0] {
                    Object::Array(a) => a.len() as i64,
                    Object::Dict(d) => d.len() as i64,
                    Object::Str(s) => s.chars().count() as i64,
                    Object::Nil => 0,
                    other => {
                        return Err(format!("{} 不支持取长度", other.kind()));
                    }
                };
                Ok(vec![Object::int(n)])
            }),
        ),
        // 追加返回**新数组**，不修改原数组：与 `数组.凡` 中
        // `结果 = 系统.追加(结果, 项)` 的用法一致。
        NativeSpec::core(
            "append",
            Some(2),
            Rc::new(|_rt, args| {
                let mut elements = match &args[0] {
                    Object::Array(a) => a.elements(),
                    other => return Err(format!("{} 不支持追加", other.kind())),
                };
                for item in &args[1..] {
                    elements.push(item.clone());
                }
                Ok(vec![Object::array(elements)])
            }),
        ),
        NativeSpec::core(
            "trunc",
            Some(1),
            Rc::new(|_rt, args| {
                let v = match &args[0] {
                    Object::Int(i) => *i as f64,
                    Object::Float(f) => *f,
                    Object::Byte(b) => *b as f64,
                    Object::Char(c) => *c as i64 as f64,
                    other => return Err(format!("{} 不能截断为整数", other.kind())),
                };
                Ok(vec![Object::int(v.trunc() as i64)])
            }),
        ),
        // 字符 → Unicode 编码点
        NativeSpec::core(
            "ord",
            Some(1),
            Rc::new(|_rt, args| match &args[0] {
                Object::Char(c) => Ok(vec![Object::int(*c as i64)]),
                Object::Str(s) => {
                    let mut it = s.chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) => Ok(vec![Object::int(c as i64)]),
                        _ => Err("编码点 需要一个字符".to_string()),
                    }
                }
                Object::Byte(b) => Ok(vec![Object::int(*b as i64)]),
                other => Err(format!("{} 不能取编码点", other.kind())),
            }),
        ),
        // 编码点 → 字符
        NativeSpec::core(
            "char",
            Some(1),
            Rc::new(|_rt, args| {
                let code = args[0].as_int().ok_or("字符 需要整数编码点")?;
                match char::from_u32(code as u32) {
                    Some(c) => Ok(vec![Object::Char(c)]),
                    None => Err(format!("{} 不是有效的编码点", code)),
                }
            }),
        ),
        // 构造错误值（不抛出），供凡语言层组装并返回。
        // 参数与 Go 版 builtinError 对齐：消息、可选错误码（字符串）、可选详情（字典）。
        NativeSpec::core(
            "error",
            Some(1),
            Rc::new(|_rt, args| {
                let message = match &args[0] {
                    Object::Str(s) => s.to_string(),
                    other => other.inspect(),
                };
                let mut value = ErrorValue::new(message);
                if let Some(second) = args.get(1) {
                    match second {
                        Object::Str(code) => value = value.with_code(code.as_ref()),
                        Object::Nil => {}
                        other => {
                            return Err(format!(
                                "错误 的第 2 个参数（错误码）必须是字符串，实际是 {}",
                                other.kind()
                            ))
                        }
                    }
                }
                if let Some(third) = args.get(2) {
                    match third {
                        Object::Dict(details) => value = value.with_details(details.clone()),
                        Object::Nil => {}
                        other => {
                            return Err(format!(
                                "错误 的第 3 个参数（详情）必须是字典，实际是 {}",
                                other.kind()
                            ))
                        }
                    }
                }
                Ok(vec![Object::Error(Rc::new(std::cell::RefCell::new(
                    value,
                )))])
            }),
        ),
        // 失败：直接以错误中断当前函数
        NativeSpec::core(
            "fail",
            Some(1),
            Rc::new(|_rt, args| {
                let message = match &args[0] {
                    Object::Str(s) => s.to_string(),
                    other => other.inspect(),
                };
                Err(message)
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::specs;
    use fan_object::{Builtin, Runtime};
    use std::rc::Rc;

    /// 按名字取出内建并调用。
    fn call(name: &str, args: Vec<fan_object::Object>) -> Result<Vec<fan_object::Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    /// 注册全部内建，返回运行实例（模拟解释器启动）。
    fn runtime() -> Runtime {
        let mut rt = Runtime::new();
        for spec in specs() {
            rt.register_native(Rc::new(Builtin::new(
                spec.name,
                spec.arity,
                spec.func.clone(),
            )));
        }
        rt
    }

    #[test]
    fn 类型名报自定义类型自己的名字() {
        // 类型名(姓名("凡")) 是「姓名」而非底层的「string」
        use fan_object::Object;
        assert_eq!(
            call("type", vec![Object::named("姓名", Object::string("凡"))]).unwrap()[0],
            Object::string("姓名")
        );
    }

    #[test]
    fn 类型名返回英文标识() {
        use fan_object::Object;
        assert_eq!(
            call("type", vec![Object::int(1)]).unwrap()[0],
            Object::string("integer")
        );
        assert_eq!(
            call("type", vec![Object::string("甲")]).unwrap()[0],
            Object::string("string")
        );
        assert_eq!(
            call("type", vec![Object::Float(1.5)]).unwrap()[0],
            Object::string("float")
        );
        assert_eq!(
            call("type", vec![Object::Bool(true)]).unwrap()[0],
            Object::string("boolean")
        );
        assert_eq!(
            call("type", vec![Object::nil()]).unwrap()[0],
            Object::string("nil")
        );
        assert_eq!(
            call("type", vec![Object::array(vec![])]).unwrap()[0],
            Object::string("array")
        );
    }

    #[test]
    fn 长度支持数组字典与字符串() {
        use fan_object::Object;
        assert_eq!(
            call(
                "len",
                vec![Object::array(vec![Object::int(1), Object::int(2)])]
            )
            .unwrap()[0],
            Object::int(2)
        );
        assert_eq!(
            call("len", vec![Object::string("甲乙丙")]).unwrap()[0],
            Object::int(3)
        );
        assert_eq!(call("len", vec![Object::nil()]).unwrap()[0], Object::int(0));
    }

    #[test]
    fn 长度对不支持的类型报错() {
        assert!(call("len", vec![fan_object::Object::int(1)])
            .unwrap_err()
            .contains("不支持取长度"));
    }

    #[test]
    fn 追加返回新数组且不改动原数组() {
        use fan_object::Object;
        let original = Object::array(vec![Object::int(1)]);
        let result = call("append", vec![original.clone(), Object::int(2)]).unwrap();
        match &result[0] {
            Object::Array(a) => assert_eq!(a.len(), 2),
            other => panic!("期望数组，实际 {}", other.inspect()),
        }
        // 原数组未被修改
        match &original {
            Object::Array(a) => assert_eq!(a.len(), 1),
            _ => unreachable!(),
        }
    }

    #[test]
    fn 追加支持一次多个元素() {
        use fan_object::Object;
        let result = call(
            "append",
            vec![
                Object::array(vec![]),
                Object::int(1),
                Object::int(2),
                Object::int(3),
            ],
        )
        .unwrap();
        match &result[0] {
            Object::Array(a) => assert_eq!(a.len(), 3),
            other => panic!("期望数组，实际 {}", other.inspect()),
        }
    }

    #[test]
    fn 截断向零取整() {
        use fan_object::Object;
        assert_eq!(
            call("trunc", vec![Object::Float(3.7)]).unwrap()[0],
            Object::int(3)
        );
        assert_eq!(
            call("trunc", vec![Object::Float(-3.7)]).unwrap()[0],
            Object::int(-3)
        );
        assert_eq!(
            call("trunc", vec![Object::Float(3.2)]).unwrap()[0],
            Object::int(3)
        );
    }

    #[test]
    fn 编码点与字符互转() {
        use fan_object::Object;
        assert_eq!(
            call("ord", vec![Object::Char('A')]).unwrap()[0],
            Object::int(65)
        );
        assert_eq!(
            call("ord", vec![Object::string("甲")]).unwrap()[0],
            Object::int(0x7532)
        );
        assert_eq!(
            call("char", vec![Object::int(65)]).unwrap()[0],
            Object::Char('A')
        );
    }

    #[test]
    fn 编码点要求单个字符() {
        assert!(call("ord", vec![fan_object::Object::string("甲乙")])
            .unwrap_err()
            .contains("一个字符"));
    }

    #[test]
    fn 无效编码点报错() {
        assert!(call("char", vec![fan_object::Object::int(-1)])
            .unwrap_err()
            .contains("有效的编码点"));
    }

    #[test]
    fn 构造错误值携带消息() {
        use fan_object::Object;
        let out = call("error", vec![Object::string("出错了")]).unwrap();
        match &out[0] {
            Object::Error(e) => assert_eq!(e.borrow().message, "出错了"),
            other => panic!("期望错误值，实际 {}", other.inspect()),
        }
    }

    #[test]
    fn 构造错误值携带错误码与详情() {
        use fan_object::{FanDict, Object};
        let mut details = FanDict::new();
        details.set(Object::string("路径"), Object::string("/x"));
        let out = call(
            "error",
            vec![
                Object::string("磁盘满"),
                Object::string("E01"),
                Object::Dict(details),
            ],
        )
        .unwrap();
        match &out[0] {
            Object::Error(e) => {
                let e = e.borrow();
                assert_eq!(e.message, "磁盘满");
                assert_eq!(e.code, "E01");
                let got = e
                    .details
                    .as_ref()
                    .and_then(|d| d.get(&Object::string("路径")));
                assert_eq!(got, Some(Object::string("/x")));
            }
            other => panic!("期望错误值，实际 {}", other.inspect()),
        }
    }

    #[test]
    fn 构造错误值错误码类型非法时报错() {
        use fan_object::Object;
        let err = call("error", vec![Object::string("m"), Object::int(1)]).unwrap_err();
        assert!(err.contains("错误码"), "实际：{err}");
    }

    #[test]
    fn 构造错误值详情类型非法时报错() {
        use fan_object::Object;
        let err = call(
            "error",
            vec![Object::string("m"), Object::Nil, Object::int(1)],
        )
        .unwrap_err();
        assert!(err.contains("详情"), "实际：{err}");
    }

    #[test]
    fn 失败以错误中断() {
        assert_eq!(
            call("fail", vec![fan_object::Object::string("中断")]).unwrap_err(),
            "中断"
        );
    }

    #[test]
    fn 打印写入运行实例的输出通道() {
        let sink = Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut rt = Runtime::with_output(sink.clone());
        let spec = specs().into_iter().find(|s| s.name == "print").unwrap();
        (spec.func)(&mut rt, vec![fan_object::Object::string("甲")]).unwrap();
        assert_eq!(String::from_utf8(sink.borrow().clone()).unwrap(), "甲");
    }

    #[test]
    fn 系统组内建全部注册可用() {
        let rt = runtime();
        for name in [
            "print", "type", "len", "append", "trunc", "ord", "char", "error", "fail",
        ] {
            assert!(rt.lookup_native(name).is_some(), "内建 {} 未注册", name);
        }
    }
}
