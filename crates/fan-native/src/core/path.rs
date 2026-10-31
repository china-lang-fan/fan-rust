//! 路径组内建：对应 `fan-sdk/文件.凡` 的路径部分。
//!
//! 这些本质上是字符串变换，但**分隔符是平台相关的**（`/` 与 `\`），凡语言
//! 无法自行获知，故由宿主提供；其余拼接规则用标准库的 `Path` 处理。

use fan_object::{NativeSpec, Object};
use std::path::{Path, MAIN_SEPARATOR};
use std::rc::Rc;

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "pathSeparator",
            Some(0),
            Rc::new(|_rt, _args| Ok(vec![Object::string(MAIN_SEPARATOR.to_string())])),
        ),
        // 拼接可变参数：首个非空且为绝对路径的部分作为起点
        NativeSpec::core(
            "pathJoin",
            Some(1),
            Rc::new(|_rt, args| {
                let mut out = std::path::PathBuf::new();
                for part in &args {
                    let text = match part {
                        Object::Str(s) => s.to_string(),
                        Object::Array(a) => {
                            // `拼路径(部分...)` 传入的是数组
                            let mut buf = std::path::PathBuf::new();
                            for item in a.elements() {
                                buf.push(item.as_str().unwrap_or_default());
                            }
                            return Ok(vec![Object::string(buf.to_string_lossy().to_string())]);
                        }
                        other => other.inspect(),
                    };
                    out.push(text);
                }
                Ok(vec![Object::string(out.to_string_lossy().to_string())])
            }),
        ),
        NativeSpec::core(
            "pathBase",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                let name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                Ok(vec![Object::string(name)])
            }),
        ),
        NativeSpec::core(
            "pathDir",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                let parent = Path::new(&path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                Ok(vec![Object::string(parent)])
            }),
        ),
        // 扩展名含前导点，如 ".txt"；无扩展名时为空文本
        NativeSpec::core(
            "pathExt",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                let ext = Path::new(&path)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                Ok(vec![Object::string(ext)])
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_object::Runtime;

    fn call(name: &str, args: Vec<Object>) -> Result<Vec<Object>, String> {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("缺少内建 {name}"));
        let mut rt = Runtime::new();
        (spec.func)(&mut rt, args)
    }

    #[test]
    fn 分隔符为平台字符() {
        let out = call("pathSeparator", vec![]).unwrap();
        assert_eq!(out[0].inspect(), MAIN_SEPARATOR.to_string());
    }

    #[test]
    fn 拼接多个部分() {
        // 凡语言 `拼路径(部分...)` 传入数组
        let parts = Object::array(vec![Object::string("甲"), Object::string("乙.txt")]);
        let out = call("pathJoin", vec![parts]).unwrap()[0].inspect();
        assert!(out.ends_with("乙.txt"), "实际：{}", out);
        assert!(out.contains("甲"), "实际：{}", out);
    }

    #[test]
    fn 拼接也接受直接传入的多个字符串() {
        let out = call("pathJoin", vec![Object::string("甲"), Object::string("乙")]).unwrap()[0]
            .inspect();
        assert!(out.contains("甲"), "实际：{}", out);
        assert!(out.contains("乙"), "实际：{}", out);
    }

    #[test]
    fn 取文件名与父目录() {
        let path = format!("甲{}乙.txt", MAIN_SEPARATOR);
        assert_eq!(
            call("pathBase", vec![Object::string(&path)]).unwrap()[0].inspect(),
            "乙.txt"
        );
        assert_eq!(
            call("pathDir", vec![Object::string(&path)]).unwrap()[0].inspect(),
            "甲"
        );
    }

    #[test]
    fn 取扩展名含前导点() {
        assert_eq!(
            call("pathExt", vec![Object::string("甲/乙.txt")]).unwrap()[0].inspect(),
            ".txt"
        );
        assert_eq!(
            call("pathExt", vec![Object::string("甲/乙")]).unwrap()[0].inspect(),
            ""
        );
    }

    #[test]
    fn 无父目录时返回空() {
        assert_eq!(
            call("pathDir", vec![Object::string("乙")]).unwrap()[0].inspect(),
            ""
        );
    }

    #[test]
    fn 路径组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "pathSeparator",
            "pathJoin",
            "pathBase",
            "pathDir",
            "pathExt",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
