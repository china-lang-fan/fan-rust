//! 进程组内建：对应 `fan-sdk/进程.凡`。
//!
//! 凡语言运行在解释器内部，无法自行读取进程上下文或启动子进程，这些能力
//! 只能由宿主提供。全部用标准库实现（`std::env`、`std::process`）。

use fan_object::{FanDict, NativeSpec, Object};
use std::rc::Rc;

use crate::core::file::err_value;

pub fn specs() -> Vec<NativeSpec> {
    vec![
        // 命令行参数不含程序名本身
        NativeSpec::core(
            "processArgs",
            Some(0),
            Rc::new(|rt, _args| {
                Ok(vec![Object::array(
                    rt.args.iter().map(Object::string).collect(),
                )])
            }),
        ),
        NativeSpec::core(
            "processScript",
            Some(0),
            Rc::new(|rt, _args| Ok(vec![Object::string(rt.script_path.clone())])),
        ),
        NativeSpec::core(
            "processEnv",
            Some(1),
            Rc::new(|_rt, args| {
                let name = args[0].as_str().unwrap_or_default();
                // 未设置的变量返回空文本，与凡语言层 `-> 字符串` 的约定一致
                Ok(vec![Object::string(
                    std::env::var(&name).unwrap_or_default(),
                )])
            }),
        ),
        NativeSpec::core(
            "processSetEnv",
            Some(2),
            Rc::new(|_rt, args| {
                let name = args[0].as_str().unwrap_or_default();
                let value = args[1].as_str().unwrap_or_default();
                // `std::env::set_var` 在 Rust 2024 起为 unsafe；此处变量由凡代码
                // 显式设置，且解释器为单线程执行，故可控。
                #[allow(unsafe_code)]
                unsafe {
                    std::env::set_var(&name, &value);
                }
                Ok(vec![Object::Nil])
            }),
        ),
        NativeSpec::core(
            "processWorkingDir",
            Some(0),
            Rc::new(|_rt, _args| match std::env::current_dir() {
                Ok(p) => Ok(vec![Object::string(p.to_string_lossy().to_string())]),
                Err(e) => Ok(vec![err_value("读取工作目录", e)]),
            }),
        ),
        NativeSpec::core(
            "processChangeDir",
            Some(1),
            Rc::new(|_rt, args| {
                let path = args[0].as_str().unwrap_or_default();
                match std::env::set_current_dir(&path) {
                    Ok(()) => Ok(vec![Object::Nil]),
                    Err(e) => Ok(vec![err_value("切换工作目录", e)]),
                }
            }),
        ),
        NativeSpec::core(
            "processPID",
            Some(0),
            Rc::new(|_rt, _args| Ok(vec![Object::int(std::process::id() as i64)])),
        ),
        // 父进程号没有跨平台的 std 接口：macOS/Linux 走 libc，Windows 返回 0
        NativeSpec::core(
            "processParentPID",
            Some(0),
            Rc::new(|_rt, _args| Ok(vec![Object::int(parent_pid() as i64)])),
        ),
        NativeSpec::core(
            "processExecute",
            Some(1),
            Rc::new(|_rt, args| {
                let command = args[0].as_str().unwrap_or_default();
                let mut cmd = std::process::Command::new(&command);
                if let Some(Object::Array(a)) = args.get(1) {
                    for item in a.elements() {
                        cmd.arg(item.as_str().unwrap_or_default());
                    }
                }
                // 选项：cwd / input / env
                let options = args.get(2).and_then(|v| match v {
                    Object::Dict(d) => Some(d.clone()),
                    _ => None,
                });
                if let Some(d) = &options {
                    if let Some(cwd) = d.get(&Object::string("cwd")).and_then(|v| v.as_str()) {
                        cmd.current_dir(cwd);
                    }
                    if let Some(Object::Dict(env)) = d.get(&Object::string("env")) {
                        for key in env.keys() {
                            if let (Some(k), Some(v)) = (key.as_str(), env.get(&key)) {
                                cmd.env(k, v.as_str().unwrap_or_default());
                            }
                        }
                    }
                }

                // 结果一律是字典：{success, exitCode, stdout, stderr}
                let mut result = FanDict::new();
                result.set(Object::string("success"), Object::Bool(false));
                result.set(Object::string("exitCode"), Object::int(-1));
                result.set(Object::string("stdout"), Object::string(String::new()));
                result.set(Object::string("stderr"), Object::string(String::new()));

                let stdin_text = options
                    .as_ref()
                    .and_then(|d| d.get(&Object::string("input")))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                let spawned = if stdin_text.is_empty() {
                    cmd.stdin(std::process::Stdio::null()).output()
                } else {
                    use std::io::Write;
                    let mut child = match cmd
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                    {
                        Ok(c) => c,
                        Err(e) => {
                            return Ok(vec![Object::Dict(result), err_value("启动命令", e)]);
                        }
                    };
                    if let Some(mut sin) = child.stdin.take() {
                        let _ = sin.write_all(stdin_text.as_bytes());
                    }
                    child.wait_with_output()
                };

                match spawned {
                    Ok(out) => {
                        let code = out.status.code().unwrap_or(-1);
                        result.set(
                            Object::string("success"),
                            Object::Bool(out.status.success()),
                        );
                        result.set(Object::string("exitCode"), Object::int(code as i64));
                        result.set(
                            Object::string("stdout"),
                            Object::string(String::from_utf8_lossy(&out.stdout).to_string()),
                        );
                        result.set(
                            Object::string("stderr"),
                            Object::string(String::from_utf8_lossy(&out.stderr).to_string()),
                        );
                        // 非零退出码不是「执行错误」：调用方通过 success 判断
                        Ok(vec![Object::Dict(result), Object::Nil])
                    }
                    // output() 自身的失败只可能是启动失败（命令不存在等）
                    Err(e) => Ok(vec![Object::Dict(result), err_value("启动命令", e)]),
                }
            }),
        ),
    ]
}

/// 取父进程号；取不到时返回 0（凡语言层据此判断有效性）。
fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        // getppid 在 libc 中，这里直接声明以避免引入依赖
        extern "C" {
            fn getppid() -> i32;
        }
        unsafe { getppid() as u32 }
    }
    #[cfg(not(unix))]
    {
        0
    }
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
    fn 进程号与父进程号为正() {
        assert!(call("processPID", vec![]).unwrap()[0].as_int().unwrap() > 0);
        let parent = call("processParentPID", vec![]).unwrap()[0]
            .as_int()
            .unwrap();
        assert!(parent > 0, "父进程号：{}", parent);
    }

    #[test]
    fn 参数取自运行实例() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "processArgs")
            .unwrap();
        let mut rt = Runtime::new();
        rt.args = vec!["甲".to_string(), "乙".to_string()];
        let out = (spec.func)(&mut rt, vec![]).unwrap();
        assert_eq!(out[0].inspect(), "[甲, 乙]");
    }

    #[test]
    fn 脚本路径取自运行实例() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "processScript")
            .unwrap();
        let mut rt = Runtime::new();
        rt.script_path = "甲.凡".to_string();
        assert_eq!((spec.func)(&mut rt, vec![]).unwrap()[0].inspect(), "甲.凡");
    }

    #[test]
    fn 读写环境变量() {
        let name = format!("FAN_NATIVE_TEST_{}", std::process::id());
        assert_eq!(
            call("processEnv", vec![Object::string(&name)]).unwrap()[0].inspect(),
            ""
        );
        assert_eq!(
            call(
                "processSetEnv",
                vec![Object::string(&name), Object::string("凡")]
            )
            .unwrap()[0],
            Object::Nil
        );
        assert_eq!(
            call("processEnv", vec![Object::string(&name)]).unwrap()[0].inspect(),
            "凡"
        );
    }

    #[test]
    fn 工作目录可读写() {
        let out = call("processWorkingDir", vec![]).unwrap();
        assert!(!out[0].inspect().is_empty());
        let cwd = out[0].inspect();
        // macOS 上 `temp_dir()` 与 `current_dir()` 的符号链接解析结果不同
        // （/var 与 /private/var），故比较规范化后的路径
        let temp = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(
            call("processChangeDir", vec![Object::string(&temp)]).unwrap()[0],
            Object::Nil
        );
        assert_eq!(
            call("processWorkingDir", vec![]).unwrap()[0].inspect(),
            temp
        );
        assert_eq!(
            call("processChangeDir", vec![Object::string(&cwd)]).unwrap()[0],
            Object::Nil
        );
    }

    #[test]
    fn 切换到不存在的目录返回错误() {
        let out = call("processChangeDir", vec![Object::string("/不存在的目录")]).unwrap();
        assert!(
            out[0].inspect().contains("切换工作目录失败"),
            "实际：{}",
            out[0].inspect()
        );
    }

    /// 取出结果字典，便于断言各字段。
    fn result_dict(out: &[Object]) -> FanDict {
        match &out[0] {
            Object::Dict(d) => d.clone(),
            other => panic!("期望结果字典，实际 {}", other.inspect()),
        }
    }

    #[test]
    fn 命令不存在时返回错误与失败结果() {
        let out = call("processExecute", vec![Object::string("绝不存在的命令")]).unwrap();
        // 即便启动失败，结果仍是字典且 success 为假：调用方统一读取
        assert_eq!(
            result_dict(&out).get(&Object::string("success")),
            Some(Object::Bool(false))
        );
        assert!(
            out[1].inspect().contains("启动命令失败"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[cfg(unix)]
    #[test]
    fn 执行命令返回结构化结果() {
        let out = call(
            "processExecute",
            vec![
                Object::string("/bin/echo"),
                Object::array(vec![Object::string("凡")]),
                Object::Nil,
            ],
        )
        .unwrap();
        assert_eq!(out[1], Object::Nil);
        let dict = result_dict(&out);
        assert_eq!(
            dict.get(&Object::string("success")),
            Some(Object::Bool(true))
        );
        assert_eq!(dict.get(&Object::string("exitCode")), Some(Object::int(0)));
        assert_eq!(
            dict.get(&Object::string("stdout")),
            Some(Object::string("凡\n".to_string()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn 非零退出码不算执行错误() {
        // `/bin/sh -c 'exit 3'`：进程正常跑完但退出码非零，
        // 应体现在 success 上，而不是抛执行错误
        let out = call(
            "processExecute",
            vec![
                Object::string("/bin/sh"),
                Object::array(vec![Object::string("-c"), Object::string("exit 3")]),
                Object::Nil,
            ],
        )
        .unwrap();
        assert_eq!(out[1], Object::Nil);
        let dict = result_dict(&out);
        assert_eq!(
            dict.get(&Object::string("success")),
            Some(Object::Bool(false))
        );
        assert_eq!(dict.get(&Object::string("exitCode")), Some(Object::int(3)));
    }

    #[cfg(unix)]
    #[test]
    fn 传入标准输入与环境变量() {
        let mut options = FanDict::new();
        options.set(Object::string("input"), Object::string("你好"));
        let mut env = FanDict::new();
        env.set(Object::string("FAN_TEST_ENV"), Object::string("凡"));
        options.set(Object::string("env"), Object::Dict(env));
        let out = call(
            "processExecute",
            vec![
                Object::string("/bin/sh"),
                Object::array(vec![
                    Object::string("-c"),
                    Object::string("printf \"%s%s\" \"$FAN_TEST_ENV\" \"$(cat)\""),
                ]),
                Object::Dict(options),
            ],
        )
        .unwrap();
        assert_eq!(out[1], Object::Nil);
        assert_eq!(
            result_dict(&out).get(&Object::string("stdout")),
            Some(Object::string("凡你好".to_string()))
        );
    }

    #[test]
    fn 进程组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "processArgs",
            "processScript",
            "processEnv",
            "processSetEnv",
            "processWorkingDir",
            "processChangeDir",
            "processPID",
            "processParentPID",
            "processExecute",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
