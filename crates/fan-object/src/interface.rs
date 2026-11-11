//! 接口满足性检查与方法签名比对。
//!
//! 对应 Go 版 `evaluator/interface.go` 的 `checkInterfaceSatisfied` 与
//! `signatureMismatch`。凡语言的接口是结构化的：模型无需声明实现，
//! 拥有全部匹配方法即自动满足。
//!
//! 规则：
//! - `空` 值视为满足任何接口；
//! - 接口中未标注的类型（空 `DeclType`）不约束；
//! - 实现方法完全未标注类型时只检查参数数量（鸭子类型）；
//! - 类型比较采用名义匹配，不做协变/逆变；
//! - 同层多个嵌入提供同名方法时标记歧义，视为不满足。

use crate::runtime_entity::{Class, Function, Interface};
use crate::{runtime_type_name, Environment, Object};
use fan_ast::{DeclType, InterfaceMethodSig, Parameter};
use std::collections::HashMap;

/// 一个方法的签名信息（接收者不计入参数）。
#[derive(Clone)]
struct MethodInfo {
    params: Vec<Parameter>,
    return_types: Vec<DeclType>,
    ambiguous: bool,
}

impl MethodInfo {
    fn from_function(func: &Function) -> Self {
        MethodInfo {
            params: func.params.clone(),
            return_types: func.return_types.clone(),
            ambiguous: false,
        }
    }

    fn from_sig(sig: &InterfaceMethodSig) -> Self {
        MethodInfo {
            params: sig.params.clone(),
            return_types: sig.return_types.clone(),
            ambiguous: false,
        }
    }

    fn ambiguous() -> Self {
        MethodInfo {
            params: Vec::new(),
            return_types: Vec::new(),
            ambiguous: true,
        }
    }
}

/// 比较两个接口方法签名是否冲突；兼容（含一致）返回空串，否则返回原因。
///
/// 与 [`signature_mismatch`] 共用鸭子类型/名义匹配规则：两边都未标注类型时
/// 仅比较参数数量；任一已标注则按名义类型逐项比对。
pub fn interface_sig_conflict(want: &InterfaceMethodSig, got: &InterfaceMethodSig) -> String {
    signature_mismatch(want, &MethodInfo::from_sig(got))
}

/// 接口方法与实现方法签名不匹配的原因；匹配返回空串。
fn signature_mismatch(want: &InterfaceMethodSig, got: &MethodInfo) -> String {
    let want_params = &want.params;
    let got_params = &got.params;

    // 实现方法完全未标注类型（参数与返回均为 Any）时，跳过签名细查。
    let impl_fully_untyped = got_params.iter().all(|p| p.type_.is_any())
        && got.return_types.iter().all(DeclType::is_any);

    let got_variadic = got_params.last().map(|p| p.variadic).unwrap_or(false);

    if impl_fully_untyped {
        if got_variadic {
            let fixed = got_params.len() - 1;
            if want_params.len() < fixed {
                return format!(
                    "参数数量不符：需要 {} 个，实际 {} 个",
                    want_params.len(),
                    got_params.len()
                );
            }
        } else if want_params.len() != got_params.len() {
            return format!(
                "参数数量不符：需要 {} 个，实际 {} 个",
                want_params.len(),
                got_params.len()
            );
        }
        return String::new();
    }

    // 可变参数：实现方法末参可变时，允许接口参数数量不小于固定参数数。
    if got_variadic {
        let fixed = got_params.len() - 1;
        if want_params.len() < fixed {
            return format!(
                "参数数量不符：需要 {} 个，实际 {} 个",
                want_params.len(),
                got_params.len()
            );
        }
    } else if want_params.len() != got_params.len() {
        return format!(
            "参数数量不符：需要 {} 个，实际 {} 个",
            want_params.len(),
            got_params.len()
        );
    }

    for (i, wp) in want_params.iter().enumerate() {
        if wp.type_.is_any() {
            continue;
        }
        let gp = if i < got_params.len() {
            &got_params[i]
        } else if got_variadic {
            got_params.last().expect("可变参数存在")
        } else {
            continue;
        };
        if gp.type_.is_any() {
            continue;
        }
        if wp.type_ != gp.type_ {
            return format!(
                "参数 {} 类型不符：需要 {}，实际 {}",
                wp.name, wp.type_, gp.type_
            );
        }
    }

    if want.return_types.is_empty() {
        return String::new();
    }
    if want.return_types.len() != got.return_types.len() {
        return format!(
            "返回值数量不符：需要 {} 个，实际 {} 个",
            want.return_types.len(),
            got.return_types.len()
        );
    }
    for (i, want_type) in want.return_types.iter().enumerate() {
        if want_type.is_any() {
            continue;
        }
        let got_type = &got.return_types[i];
        if got_type.is_any() {
            continue;
        }
        if want_type != got_type {
            return format!("返回值类型不符：需要 {}，实际 {}", want_type, got_type);
        }
    }
    String::new()
}

/// 收集模型自身及嵌入提升后的方法；同层多个嵌入提供同名方法时标记歧义。
fn class_method_set(cls: &Class, seen: &mut Vec<String>) -> HashMap<String, MethodInfo> {
    let mut out: HashMap<String, MethodInfo> = HashMap::new();
    if seen.iter().any(|s| s == &cls.name) {
        return out;
    }
    seen.push(cls.name.clone());

    for func in cls.methods.values() {
        out.insert(func.name.clone(), MethodInfo::from_function(func));
    }

    let mut embed_sets: Vec<HashMap<String, MethodInfo>> = Vec::new();
    for embed in &cls.embeds {
        embed_sets.push(class_method_set(embed, seen));
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for set in &embed_sets {
        for (name, info) in set {
            if info.ambiguous {
                out.insert(name.clone(), info.clone());
                counts.insert(name.clone(), 2);
                continue;
            }
            counts
                .entry(name.clone())
                .and_modify(|n| *n += 1)
                .or_insert(1);
            out.entry(name.clone()).or_insert_with(|| info.clone());
        }
    }
    for (name, n) in counts {
        if n >= 2 {
            out.insert(name, MethodInfo::ambiguous());
        }
    }
    out
}

/// 收集自定义命名类型上定义的方法（登记在 primitive_methods，键为类型中文名）。
fn named_method_set(type_name: &str, env: &Environment) -> HashMap<String, MethodInfo> {
    let mut out = HashMap::new();
    let rt = env.runtime();
    let rt_ref = rt.borrow();
    if let Some(methods) = rt_ref.primitive_methods.get(type_name) {
        for (name, func) in methods {
            out.insert(name.clone(), MethodInfo::from_function(func));
        }
    }
    out
}

/// 按运行时类型标识收集基本类型的扩展方法。
fn primitive_method_set(value: &Object, env: &Environment) -> HashMap<String, MethodInfo> {
    let mut out = HashMap::new();
    let kind = runtime_type_name(value);
    let rt = env.runtime();
    let rt_ref = rt.borrow();
    if let Some(methods) = rt_ref.primitive_methods.get(kind) {
        for (name, func) in methods {
            out.insert(name.clone(), MethodInfo::from_function(func));
        }
    }
    out
}

/// 收集一个值可调用的全部方法（模型/命名类型/基本类型）。
fn value_method_set(value: &Object, env: &Environment) -> HashMap<String, MethodInfo> {
    match value {
        Object::Instance(inst) => class_method_set(&inst.class, &mut Vec::new()),
        Object::Named(named) => {
            let mut methods = named_method_set(&named.type_name, env);
            match &named.value {
                Object::Instance(inst) => {
                    let inner_set = class_method_set(&inst.class, &mut Vec::new());
                    for (name, info) in inner_set {
                        methods.entry(name).or_insert(info);
                    }
                }
                inner => {
                    let inner_set = primitive_method_set(inner, env);
                    for (name, info) in inner_set {
                        methods.entry(name).or_insert(info);
                    }
                }
            }
            methods
        }
        other => primitive_method_set(other, env),
    }
}

/// 返回第一个不满足的接口方法名及原因；全部满足返回 `None`。
///
/// `空` 值视为满足任何接口（与 Go 的 nil 接口一致）。
pub fn interface_missing_method(
    iface: &Interface,
    value: &Object,
    env: &Environment,
) -> Option<(String, String)> {
    if iface.methods.is_empty() || matches!(value, Object::Nil) {
        return None;
    }
    let checked = value.unwrap_named();
    let methods = value_method_set(&checked, env);
    let mut names: Vec<&String> = iface.methods.keys().collect();
    names.sort();
    for name in names {
        let want = iface.methods.get(name).expect("接口方法存在");
        let Some(got) = methods.get(name) else {
            return Some((name.clone(), "缺少".to_string()));
        };
        if got.ambiguous {
            return Some((name.clone(), "嵌入链上存在同名歧义方法".to_string()));
        }
        let reason = signature_mismatch(want, got);
        if !reason.is_empty() {
            return Some((name.clone(), reason));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use fan_ast::{BlockStmt, Position};
    use std::rc::Rc;

    fn env() -> Environment {
        Environment::new()
    }

    fn iface_with(methods: Vec<InterfaceMethodSig>) -> Interface {
        let mut iface = Interface::new("测试接口");
        for m in methods {
            iface.methods.insert(m.name.clone(), m);
        }
        iface
    }

    fn sig(name: &str, params: Vec<Parameter>, returns: Vec<DeclType>) -> InterfaceMethodSig {
        InterfaceMethodSig {
            name: name.to_string(),
            params,
            return_types: returns,
            pos: Position::START,
        }
    }

    fn param(name: &str, type_: DeclType) -> Parameter {
        Parameter {
            name: name.to_string(),
            type_,
            variadic: false,
            default: None,
        }
    }

    fn model_with(methods: Vec<Function>) -> Object {
        let mut cls = Class::new("实现模型");
        for f in methods {
            cls.methods.insert(f.name.clone(), Rc::new(f));
        }
        let inst = crate::Instance::new(Rc::new(cls));
        Object::Instance(Rc::new(inst))
    }

    fn function(params: Vec<Parameter>, returns: Vec<DeclType>) -> Function {
        let mut f = Function::new(
            "处理",
            Vec::new(),
            BlockStmt::default(),
            None,
            Position::START,
        );
        f.params = params;
        f.return_types = returns;
        f
    }

    #[test]
    fn 参数类型不符时不满足() {
        let iface = iface_with(vec![sig(
            "处理",
            vec![param("内容", DeclType::named("字符串"))],
            vec![DeclType::named("字符串")],
        )]);
        let value = model_with(vec![function(
            vec![param("内容", DeclType::named("整数"))],
            vec![DeclType::named("字符串")],
        )]);
        let (method, reason) = interface_missing_method(&iface, &value, &env()).expect("应不满足");
        assert_eq!(method, "处理");
        assert!(reason.contains("类型不符"), "实际原因：{reason}");
    }

    #[test]
    fn 返回值类型不符时不满足() {
        let iface = iface_with(vec![sig(
            "处理",
            Vec::new(),
            vec![DeclType::named("字符串")],
        )]);
        let value = model_with(vec![function(Vec::new(), vec![DeclType::named("整数")])]);
        let (_, reason) = interface_missing_method(&iface, &value, &env()).expect("应不满足");
        assert!(reason.contains("返回值类型不符"), "实际原因：{reason}");
    }

    #[test]
    fn 参数数量不符时不满足() {
        let iface = iface_with(vec![sig(
            "处理",
            vec![param("a", DeclType::any()), param("b", DeclType::any())],
            Vec::new(),
        )]);
        let value = model_with(vec![function(
            vec![param("a", DeclType::any())],
            Vec::new(),
        )]);
        let (_, reason) = interface_missing_method(&iface, &value, &env()).expect("应不满足");
        assert!(reason.contains("参数数量不符"), "实际原因：{reason}");
    }

    #[test]
    fn 签名一致时满足() {
        let iface = iface_with(vec![sig(
            "处理",
            vec![param("内容", DeclType::named("字符串"))],
            vec![DeclType::named("字符串")],
        )]);
        let value = model_with(vec![function(
            vec![param("内容", DeclType::named("字符串"))],
            vec![DeclType::named("字符串")],
        )]);
        assert_eq!(interface_missing_method(&iface, &value, &env()), None);
    }

    #[test]
    fn 实现完全未标注类型时只查参数数量() {
        let iface = iface_with(vec![sig(
            "处理",
            vec![param("a", DeclType::named("整数"))],
            vec![DeclType::named("字符串")],
        )]);
        // 鸭子类型：实现方参数与返回均未标注，只要数量一致即满足
        let value = model_with(vec![function(
            vec![param("a", DeclType::any())],
            Vec::new(),
        )]);
        assert_eq!(interface_missing_method(&iface, &value, &env()), None);
    }

    #[test]
    fn 接口未标注的类型项不约束() {
        let iface = iface_with(vec![sig(
            "处理",
            vec![param("a", DeclType::any())],
            Vec::new(),
        )]);
        let value = model_with(vec![function(
            vec![param("a", DeclType::named("整数"))],
            Vec::new(),
        )]);
        assert_eq!(interface_missing_method(&iface, &value, &env()), None);
    }

    #[test]
    fn 空值满足任何接口() {
        let iface = iface_with(vec![sig("处理", Vec::new(), Vec::new())]);
        assert_eq!(interface_missing_method(&iface, &Object::Nil, &env()), None);
    }

    #[test]
    fn 缺少方法时返回缺少原因() {
        let iface = iface_with(vec![sig("处理", Vec::new(), Vec::new())]);
        let value = model_with(Vec::new());
        let (method, reason) = interface_missing_method(&iface, &value, &env()).expect("应不满足");
        assert_eq!(method, "处理");
        assert_eq!(reason, "缺少");
    }
}
