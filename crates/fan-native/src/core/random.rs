//! 随机组内建：整数与小数随机值。
//!
//! 对应 `fan-sdk/随机.凡`。刻意**不引入** `rand` 等第三方库：随机数发生器
//! 用 PCG32 实现（约二十行、纯标准库），种子取自系统时间，避免把 `rand`
//! 及其依赖树编入解释器。

use fan_object::{NativeSpec, Object};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 由系统时间生成初始种子。
fn seed_from_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x853C_49E6_748F_EA9B)
}

thread_local! {
    /// 发生器状态。用 `Cell` 而非 `RefCell`：取值与更新都不会失败。
    static STATE: Cell<u64> = Cell::new(seed_from_time());
}

/// PCG32 的一步推进，返回 32 位无符号随机数。
fn next_u32() -> u32 {
    const MULT: u64 = 6364136223846793005;
    const INC: u64 = 1442695040888963407;
    STATE.with(|s| {
        let old = s.get();
        s.set(old.wrapping_mul(MULT).wrapping_add(INC));
        // 输出混淆：高位参与旋转，低位决定旋转量
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    })
}

/// `[0, 1)` 区间的随机小数。
fn next_f64() -> f64 {
    (next_u32() as f64) / (u32::MAX as f64 + 1.0)
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        // 随机整数(最大值)：返回 `[0, 最大值)`，与 `随机_测试.凡` 的断言一致
        NativeSpec::core(
            "randomInt",
            Some(1),
            Rc::new(|_rt, args| {
                let max = args[0].as_int().ok_or("随机整数 需要整数上界")?;
                if max <= 0 {
                    return Err("随机整数 的上界必须为正".to_string());
                }
                // next_f64() < 1.0，乘积截断后必然 < max
                Ok(vec![Object::int((next_f64() * max as f64) as i64)])
            }),
        ),
        // 随机小数()：返回 `[0, 1)`
        NativeSpec::core(
            "randomFloat",
            Some(0),
            Rc::new(|_rt, _args| Ok(vec![Object::Float(next_f64())])),
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
    fn 随机整数落在区间内() {
        // 与 随机_测试.凡 的断言一致：0 <= 值 < 10
        for _ in 0..200 {
            let v = call("randomInt", vec![Object::int(10)]).unwrap()[0]
                .as_int()
                .unwrap();
            assert!((0..10).contains(&v), "越界：{}", v);
        }
    }

    #[test]
    fn 随机小数落在零到一之间() {
        for _ in 0..200 {
            let v = call("randomFloat", vec![]).unwrap()[0].as_float().unwrap();
            assert!((0.0..1.0).contains(&v), "越界：{}", v);
        }
    }

    #[test]
    fn 上界为一时恒为零() {
        for _ in 0..20 {
            assert_eq!(
                call("randomInt", vec![Object::int(1)]).unwrap()[0],
                Object::int(0)
            );
        }
    }

    #[test]
    fn 非正上界报错() {
        assert!(call("randomInt", vec![Object::int(0)])
            .unwrap_err()
            .contains("必须为正"));
        assert!(call("randomInt", vec![Object::int(-5)])
            .unwrap_err()
            .contains("必须为正"));
    }

    #[test]
    fn 连续取值不全相同() {
        // 发生器确实在推进：200 次取值应出现多个不同结果
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            seen.insert(call("randomInt", vec![Object::int(1000)]).unwrap()[0].inspect());
        }
        assert!(
            seen.len() > 10,
            "随机序列过于重复，仅 {} 种取值",
            seen.len()
        );
    }

    #[test]
    fn 发生器推进不溢出() {
        // 连续取一万次不应 panic（验证 wrapping 运算正确）
        for _ in 0..10_000 {
            let _ = next_u32();
        }
    }
}
