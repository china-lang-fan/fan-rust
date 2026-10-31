//! 文件组内建：对应 `fan-sdk/文件.凡` 的宿主能力部分。
//!
//! 凡语言无法自行触达文件系统，故这些能力必须由宿主提供。凡语言层负责
//! 组装（`复制`、`移动`、`确保目录` 等由凡代码实现），内建只提供原子操作。
//!
//! 约定：失败时返回**错误值**而非抛错，与凡语言层 `返回 空, 错误` 的写法一致；
//! 成功的操作类函数返回 `空`。

use fan_object::{ErrorValue, FanDict, NativeSpec, Object};
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

/// 把标准库错误转成凡语言错误值。
pub(crate) fn err_value(op: &str, e: std::io::Error) -> Object {
    Object::Error(Rc::new(std::cell::RefCell::new(ErrorValue::new(format!(
        "{}失败：{}",
        op, e
    )))))
}

/// 操作类函数的返回：成功为空，失败为错误值。
fn op_result(op: &str, r: std::io::Result<()>) -> Vec<Object> {
    match r {
        Ok(()) => vec![Object::Nil],
        Err(e) => vec![err_value(op, e)],
    }
}

thread_local! {
    /// 临时名计数器，与进程号、时间戳共同保证唯一性。
    static TEMP_SEQ: Cell<u64> = const { Cell::new(0) };
}

/// 生成唯一的临时条目名，并创建它（`is_dir` 决定是目录还是文件）。
fn make_temp(prefix: &str, is_dir: bool) -> std::io::Result<String> {
    let seq = TEMP_SEQ.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = format!("{}{}_{}_{}", prefix, std::process::id(), nanos, seq);
    let path = std::env::temp_dir().join(name);
    if is_dir {
        std::fs::create_dir_all(&path)?;
    } else {
        std::fs::File::create(&path)?;
    }
    // 规范化：macOS 上 `temp_dir()` 给出 /var，而 `current_dir()` 给出
    // /private/var，不规范化会让「切换目录后读工作目录」这类比较失败
    Ok(std::fs::canonicalize(&path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string()))
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        // ---------------------------------------------------------- 读写
        NativeSpec::core(
            "readFile",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                match std::fs::read_to_string(&path) {
                    Ok(text) => Ok(vec![Object::string(text), Object::Nil]),
                    Err(e) => Ok(vec![
                        Object::string(String::new()),
                        err_value("读取文件", e),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "writeFile",
            Some(2),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                let content = args[1].as_str().unwrap_or_default();
                Ok(op_result("写入文件", std::fs::write(&path, content)))
            }),
        ),
        NativeSpec::core(
            "appendFile",
            Some(2),
            Rc::new(|_rt, args| {
                use std::io::Write;
                let path = args[0].as_str().unwrap_or_default();
                let content = args[1].as_str().unwrap_or_default();
                Ok(op_result(
                    "追加文件",
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut f| f.write_all(content.as_bytes())),
                ))
            }),
        ),
        NativeSpec::core(
            "exists",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                Ok(vec![Object::Bool(Path::new(&path).exists())])
            }),
        ),
        // ---------------------------------------------------------- 增删改名
        NativeSpec::core(
            "remove",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                let p = Path::new(&path);
                let result = if p.is_dir() {
                    std::fs::remove_dir(p)
                } else {
                    std::fs::remove_file(p)
                };
                Ok(op_result("删除条目", result))
            }),
        ),
        NativeSpec::core(
            "removeAll",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                let p = Path::new(&path);
                let result = if p.is_dir() {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_file(p)
                };
                Ok(op_result("删除全部条目", result))
            }),
        ),
        NativeSpec::core(
            "rename",
            Some(2),
            Rc::new(|_rt, args| {
                let old = args[0].as_str().unwrap_or_default();
                let new = args[1].as_str().unwrap_or_default();
                Ok(op_result("重命名条目", std::fs::rename(&old, &new)))
            }),
        ),
        NativeSpec::core(
            "makeDir",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                // 第二参数为「递归」；未给出时按递归处理（更常用且安全）
                let recursive = args
                    .get(1)
                    .and_then(|v| match v {
                        Object::Bool(b) => Some(*b),
                        _ => None,
                    })
                    .unwrap_or(true);
                let result = if recursive {
                    std::fs::create_dir_all(&path)
                } else {
                    std::fs::create_dir(&path)
                };
                Ok(op_result("创建目录", result))
            }),
        ),
        NativeSpec::core(
            "listDir",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut names: Vec<Object> = Vec::new();
                        for entry in entries {
                            match entry {
                                Ok(e) => {
                                    // 目录项带分隔符后缀，便于一眼区分类型
                                    let mut name = e.file_name().to_string_lossy().to_string();
                                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                        name.push(std::path::MAIN_SEPARATOR);
                                    }
                                    names.push(Object::string(name));
                                }
                                Err(e) => {
                                    return Ok(vec![
                                        Object::array(vec![]),
                                        err_value("列出目录", e),
                                    ])
                                }
                            }
                        }
                        names.sort_by_key(|a| a.inspect());
                        Ok(vec![Object::array(names), Object::Nil])
                    }
                    Err(e) => Ok(vec![Object::array(vec![]), err_value("列出目录", e)]),
                }
            }),
        ),
        // ---------------------------------------------------------- 信息
        NativeSpec::core(
            "stat",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let mut info = FanDict::new();
                        let name = Path::new(&path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        info.set(Object::string("name"), Object::string(name));
                        info.set(Object::string("isDir"), Object::Bool(meta.is_dir()));
                        info.set(Object::string("isFile"), Object::Bool(meta.is_file()));
                        info.set(Object::string("size"), Object::int(meta.len() as i64));
                        let mtime = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        info.set(Object::string("modTime"), Object::int(mtime));
                        Ok(vec![Object::Dict(info), Object::Nil])
                    }
                    Err(e) => Ok(vec![Object::Nil, err_value("读取条目信息", e)]),
                }
            }),
        ),
        // ---------------------------------------------------------- 临时条目
        NativeSpec::core(
            "tempDir",
            Some(0),
            Rc::new(|_rt, args| {
                let prefix = args.first().and_then(|v| v.as_str()).unwrap_or_default();
                match make_temp(&prefix, true) {
                    Ok(path) => Ok(vec![Object::string(path), Object::Nil]),
                    Err(e) => Ok(vec![
                        Object::string(String::new()),
                        err_value("创建临时目录", e),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "tempFile",
            Some(0),
            Rc::new(|_rt, args| {
                let prefix = args.first().and_then(|v| v.as_str()).unwrap_or_default();
                match make_temp(&prefix, false) {
                    Ok(path) => Ok(vec![Object::string(path), Object::Nil]),
                    Err(e) => Ok(vec![
                        Object::string(String::new()),
                        err_value("创建临时文件", e),
                    ]),
                }
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

    /// 建一个临时目录，测试结束删除。
    fn temp_root() -> String {
        let out = call("tempDir", vec![Object::string("fan-native-")]).unwrap();
        assert_eq!(out[1], Object::Nil);
        out[0].inspect()
    }

    #[test]
    fn 写入读取往返() {
        let root = temp_root();
        let path = format!("{}/a.txt", root);
        assert_eq!(
            call(
                "writeFile",
                vec![Object::string(&path), Object::string("甲")]
            )
            .unwrap()[0],
            Object::Nil
        );
        let out = call("readFile", vec![Object::string(&path)]).unwrap();
        assert_eq!(out[0].inspect(), "甲");
        assert_eq!(out[1], Object::Nil);
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 追加内容() {
        let root = temp_root();
        let path = format!("{}/b.txt", root);
        call(
            "writeFile",
            vec![Object::string(&path), Object::string("甲")],
        )
        .unwrap();
        assert_eq!(
            call(
                "appendFile",
                vec![Object::string(&path), Object::string("乙")]
            )
            .unwrap()[0],
            Object::Nil
        );
        assert_eq!(
            call("readFile", vec![Object::string(&path)]).unwrap()[0].inspect(),
            "甲乙"
        );
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 读取不存在的文件返回错误而非中断() {
        let out = call("readFile", vec![Object::string("/不存在/的路径.txt")]).unwrap();
        assert_eq!(out[0].inspect(), "");
        assert!(
            out[1].inspect().contains("读取文件失败"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 存在性判断() {
        let root = temp_root();
        assert_eq!(
            call("exists", vec![Object::string(&root)]).unwrap()[0],
            Object::Bool(true)
        );
        assert_eq!(
            call("exists", vec![Object::string("/不存在/的路径")]).unwrap()[0],
            Object::Bool(false)
        );
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 创建目录与列表() {
        let root = temp_root();
        let sub = format!("{}/子目录", root);
        assert_eq!(
            call("makeDir", vec![Object::string(&sub)]).unwrap()[0],
            Object::Nil
        );
        assert_eq!(
            call("exists", vec![Object::string(&sub)]).unwrap()[0],
            Object::Bool(true)
        );
        let out = call("listDir", vec![Object::string(&root)]).unwrap();
        assert_eq!(out[1], Object::Nil);
        assert!(
            out[0].inspect().contains("子目录"),
            "实际：{}",
            out[0].inspect()
        );
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 列出不存在的目录返回错误() {
        let out = call("listDir", vec![Object::string("/不存在/的路径")]).unwrap();
        assert!(out[1].inspect().contains("列出目录失败"));
    }

    #[test]
    fn 重命名() {
        let root = temp_root();
        let src = format!("{}/c.txt", root);
        let dst = format!("{}/d.txt", root);
        call(
            "writeFile",
            vec![Object::string(&src), Object::string("内容")],
        )
        .unwrap();
        assert_eq!(
            call("rename", vec![Object::string(&src), Object::string(&dst)]).unwrap()[0],
            Object::Nil
        );
        assert_eq!(
            call("exists", vec![Object::string(&src)]).unwrap()[0],
            Object::Bool(false)
        );
        assert_eq!(
            call("exists", vec![Object::string(&dst)]).unwrap()[0],
            Object::Bool(true)
        );
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 删除单个文件() {
        let root = temp_root();
        let path = format!("{}/e.txt", root);
        call(
            "writeFile",
            vec![Object::string(&path), Object::string("甲")],
        )
        .unwrap();
        assert_eq!(
            call("remove", vec![Object::string(&path)]).unwrap()[0],
            Object::Nil
        );
        assert_eq!(
            call("exists", vec![Object::string(&path)]).unwrap()[0],
            Object::Bool(false)
        );
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 条目信息含是否目录() {
        let root = temp_root();
        let out = call("stat", vec![Object::string(&root)]).unwrap();
        assert_eq!(out[1], Object::Nil);
        let info = out[0].inspect();
        assert!(info.contains("isDir: 真"), "实际：{}", info);
        call("removeAll", vec![Object::string(&root)]).unwrap();
    }

    #[test]
    fn 临时目录与文件均创建成功() {
        let dir = call("tempDir", vec![Object::string("fan-native-")]).unwrap();
        assert_eq!(dir[1], Object::Nil);
        assert_eq!(
            call("exists", vec![dir[0].clone()]).unwrap()[0],
            Object::Bool(true)
        );

        let file = call("tempFile", vec![Object::string("fan-native-")]).unwrap();
        assert_eq!(file[1], Object::Nil);
        assert_eq!(
            call("exists", vec![file[0].clone()]).unwrap()[0],
            Object::Bool(true)
        );

        call("removeAll", vec![dir[0].clone()]).unwrap();
        call("remove", vec![file[0].clone()]).unwrap();
    }

    #[test]
    fn 临时名不重复() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let out = call("tempDir", vec![Object::string("fan-native-")]).unwrap();
            seen.insert(out[0].inspect());
            call("removeAll", vec![out[0].clone()]).unwrap();
        }
        assert_eq!(seen.len(), 20, "临时目录名出现重复");
    }

    #[test]
    fn 文件组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "readFile",
            "writeFile",
            "appendFile",
            "exists",
            "remove",
            "removeAll",
            "rename",
            "makeDir",
            "listDir",
            "stat",
            "tempDir",
            "tempFile",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
