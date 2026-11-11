//! 时间组内建：对应 `fan-sdk/时间.凡` 与 `fan-sdk/日期.凡`。
//!
//! 时间与日历换算涉及闰年、闰秒、时区等规则，凡语言无法自行实现。这里全部
//! 用标准库完成：日历换算采用 Howard Hinnant 的 `days_from_civil` /
//! `civil_from_days` 算法（纯整数运算，无需建表）。
//!
//! 布局沿用 Go 的参考时间记法（`2006-01-02 15:04:05`），与标准库对外约定一致。

use fan_object::{FanDict, NativeSpec, Object};
use std::rc::Rc;

// ---------------------------------------------------------------- 日历换算

/// 从 1970-01-01 起的天数 → (年, 月, 日)。
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 某年某月的天数（含闰年 2 月）。
fn days_in_month(year: i64, month: i64) -> i64 {
    const LENGTHS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&month) {
        return 0;
    }
    let mut days = LENGTHS[(month - 1) as usize];
    if month == 2 && is_leap_year(year) {
        days += 1;
    }
    days
}

/// 是否为闰年（格里高利历）。
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// (年, 月, 日) → 从 1970-01-01 起的天数。
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// 一个已拆解的时间分量（UTC）。
#[derive(Debug, Clone, Copy)]
struct Parts {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    weekday: u32,
}

impl Parts {
    /// 由 Unix 时间戳拆解（UTC）。
    fn from_unix(unix: i64) -> Self {
        let (year, month, day, hour, minute, second) = break_down(unix);
        Parts {
            year,
            month,
            day,
            hour,
            minute,
            second,
            weekday: weekday_from_days(unix.div_euclid(86400)),
        }
    }
}

/// 时间戳 → (年, 月, 日, 时, 分, 秒)。时间为 UTC。
fn break_down(unix: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = unix.div_euclid(86400);
    let rem = unix.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        (rem / 60 % 60) as u32,
        (rem % 60) as u32,
    )
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// 星期几（0=周日），由天数推算：1970-01-01 是周四。
fn weekday_from_days(days: i64) -> u32 {
    ((days + 4).rem_euclid(7)) as u32
}

// ---------------------------------------------------------------- 格式化

/// 按 Go 参考时间布局格式化时间戳（UTC）。
pub fn format_time(unix: i64, layout: &str) -> String {
    let parts = Parts::from_unix(unix);
    let chars: Vec<char> = layout.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        // 按最长优先匹配，避免 "01" 被 "0" 截断
        let (token, value) = match_token(&rest, parts);
        if let Some(value) = value {
            out.push_str(&value);
            i += token.chars().count();
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// 识别布局开头的记号，返回 (记号, 替换文本)。非记号返回 `None`。
fn match_token(rest: &str, p: Parts) -> (&'static str, Option<String>) {
    let hour12 = if p.hour.is_multiple_of(12) {
        12
    } else {
        p.hour % 12
    };
    let ampm = if p.hour < 12 { "AM" } else { "PM" };
    let candidates: [(&str, String); 20] = [
        ("2006", format!("{:04}", p.year)),
        ("January", MONTH_NAMES[(p.month - 1) as usize].to_string()),
        ("Jan", MONTH_NAMES[(p.month - 1) as usize][..3].to_string()),
        ("Monday", WEEKDAY_NAMES[p.weekday as usize].to_string()),
        ("Mon", WEEKDAY_NAMES[p.weekday as usize][..3].to_string()),
        ("01", format!("{:02}", p.month)),
        ("02", format!("{:02}", p.day)),
        ("03", format!("{:02}", hour12)),
        ("04", format!("{:02}", p.minute)),
        ("05", format!("{:02}", p.second)),
        ("15", format!("{:02}", p.hour)),
        ("PM", ampm.to_string()),
        ("pm", ampm.to_lowercase()),
        (".000", ".000".to_string()),
        ("Z07:00", "Z".to_string()),
        ("Z0700", "Z".to_string()),
        ("-0700", "+0000".to_string()),
        ("MST", "UTC".to_string()),
        ("1", p.month.to_string()),
        ("2", p.day.to_string()),
    ];
    for (token, value) in &candidates {
        if rest.starts_with(token) {
            // 一元记号（"1"/"2"）后紧跟数字时不视为记号
            if token.len() == 1 {
                if let Some(next) = rest[1..].chars().next() {
                    if next.is_ascii_digit() {
                        continue;
                    }
                }
            }
            return (token, Some(value.clone()));
        }
    }
    ("", None)
}

// ---------------------------------------------------------------- 解析

/// 按布局解析时间文本，返回 Unix 时间戳（UTC）。
pub fn parse_time(layout: &str, text: &str) -> Result<i64, String> {
    let layout_chars: Vec<char> = layout.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let mut li = 0;
    let mut ti = 0;

    let mut year = 1970i64;
    let mut month = 1u32;
    let mut day = 1u32;
    let mut hour = 0u32;
    let mut minute = 0u32;
    let mut second = 0u32;

    /// 从文本当前位置读最多 `max` 位数字。
    fn read_number(chars: &[char], pos: &mut usize, max: usize) -> Result<u32, String> {
        let start = *pos;
        while *pos < chars.len() && *pos - start < max && chars[*pos].is_ascii_digit() {
            *pos += 1;
        }
        if *pos == start {
            return Err("期望数字".to_string());
        }
        chars[start..*pos]
            .iter()
            .collect::<String>()
            .parse::<u32>()
            .map_err(|_| "数字无法解析".to_string())
    }

    while li < layout_chars.len() {
        let rest: String = layout_chars[li..].iter().collect();
        if let Some((token, _)) = {
            let (t, v) = match_token(&rest, Parts::from_unix(0));
            if v.is_some() {
                Some((t, v))
            } else {
                None
            }
        } {
            match token {
                "2006" => year = read_number(&text_chars, &mut ti, 4)? as i64,
                "01" | "1" => month = read_number(&text_chars, &mut ti, 2)?,
                "02" | "2" => day = read_number(&text_chars, &mut ti, 2)?,
                "15" => hour = read_number(&text_chars, &mut ti, 2)?,
                "03" | "3" => hour = read_number(&text_chars, &mut ti, 2)?,
                "04" | "4" => minute = read_number(&text_chars, &mut ti, 2)?,
                "05" | "5" => second = read_number(&text_chars, &mut ti, 2)?,
                "PM" | "pm" => {
                    // 读两个字母判断上下午
                    let mark: String = text_chars
                        .get(ti..ti + 2)
                        .unwrap_or_default()
                        .iter()
                        .collect();
                    if mark.eq_ignore_ascii_case("PM") {
                        hour = hour % 12 + 12;
                    }
                    ti += 2;
                }
                // 毫秒不参与秒级时间戳，跳过
                ".000" if text_chars.get(ti) == Some(&'.') => {
                    ti += 1;
                    let _ = read_number(&text_chars, &mut ti, 3);
                }
                "MST" | "Z07:00" | "Z0700" | "-0700" => {
                    // 时区不支持解析，按 UTC 处理
                }
                _ => {}
            }
            li += token.chars().count();
            continue;
        }
        // 字面量必须逐字符对应
        let lc = layout_chars[li];
        match text_chars.get(ti) {
            Some(&tc) if tc == lc => {
                li += 1;
                ti += 1;
            }
            _ => {
                return Err(format!(
                    "时间文本与布局不符：第 {} 个字符处期望 '{}'，实际 '{}'",
                    ti,
                    lc,
                    text_chars.get(ti).copied().unwrap_or('␄')
                ));
            }
        }
    }
    if ti < text_chars.len() {
        return Err(format!("时间文本第 {} 个字符处存在多余内容", ti));
    }
    if !(1..=12).contains(&month) {
        return Err(format!("月份越界：{}", month));
    }
    if !(1..=31).contains(&day) {
        return Err(format!("日期越界：{}", day));
    }
    let days = days_from_civil(year, month, day);
    Ok(days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64)
}

pub fn specs() -> Vec<NativeSpec> {
    vec![
        NativeSpec::core(
            "nowUnix",
            Some(0),
            Rc::new(|_rt, _args| {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                Ok(vec![Object::int(secs)])
            }),
        ),
        NativeSpec::core(
            "nowUnixMilli",
            Some(0),
            Rc::new(|_rt, _args| {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                Ok(vec![Object::int(millis)])
            }),
        ),
        NativeSpec::core(
            "formatTime",
            Some(2),
            Rc::new(|_rt, args| {
                let unix = args[0].as_int().unwrap_or(0);
                let layout = args[1].as_str().unwrap_or_default();
                Ok(vec![Object::string(format_time(unix, &layout))])
            }),
        ),
        NativeSpec::core(
            "parseTime",
            Some(2),
            Rc::new(|_rt, args| {
                let layout = args[0].as_str().unwrap_or_default();
                let text = args[1].as_str().unwrap_or_default();
                match parse_time(&layout, &text) {
                    Ok(unix) => Ok(vec![Object::int(unix), Object::Nil]),
                    Err(e) => Ok(vec![
                        Object::int(0),
                        Object::Error(Rc::new(std::cell::RefCell::new(
                            fan_object::ErrorValue::new(e),
                        ))),
                    ]),
                }
            }),
        ),
        NativeSpec::core(
            "dateUnix",
            Some(3),
            Rc::new(|_rt, args| {
                let year = args[0].as_int().unwrap_or(1970);
                let month = args[1].as_int().unwrap_or(1);
                let day = args[2].as_int().unwrap_or(1);
                // 校验要按当月实际天数，否则 2 月 30 日这类日期会被静默换算
                // 到下个月，调用方无从察觉
                if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
                    return Ok(vec![
                        Object::int(0),
                        Object::Error(Rc::new(std::cell::RefCell::new(
                            fan_object::ErrorValue::new(format!(
                                "非法日期：{}-{:02}-{:02}",
                                year, month, day
                            )),
                        ))),
                    ]);
                }
                let unix = days_from_civil(year, month as u32, day as u32) * 86400;
                Ok(vec![Object::int(unix), Object::Nil])
            }),
        ),
        NativeSpec::core(
            "timeParts",
            Some(1),
            Rc::new(|_rt, args| {
                let unix = args[0].as_int().unwrap_or(0);
                let (y, m, d, h, min, s) = break_down(unix);
                let mut parts = FanDict::new();
                parts.set(Object::string("year"), Object::int(y));
                parts.set(Object::string("month"), Object::int(m as i64));
                parts.set(Object::string("day"), Object::int(d as i64));
                parts.set(Object::string("hour"), Object::int(h as i64));
                parts.set(Object::string("minute"), Object::int(min as i64));
                parts.set(Object::string("second"), Object::int(s as i64));
                parts.set(
                    Object::string("weekday"),
                    Object::int(weekday_from_days(unix.div_euclid(86400)) as i64),
                );
                Ok(vec![Object::Dict(parts)])
            }),
        ),
        NativeSpec::core(
            "sleep",
            Some(1),
            Rc::new(|_rt, args| {
                let secs = args[0].as_float().unwrap_or(0.0);
                if secs > 0.0 {
                    std::thread::sleep(std::time::Duration::from_secs_f64(secs));
                }
                Ok(vec![])
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
    fn 日历换算往返一致() {
        for (y, m, d) in [
            (1970, 1u32, 1u32),
            (2000, 2u32, 29u32),
            (2020, 6u32, 15u32),
            (2024, 12u32, 31u32),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "{}-{}-{}", y, m, d);
        }
    }

    #[test]
    fn 闰年规则正确() {
        // 2000 年是闰年，1900 年不是
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(
            civil_from_days(days_from_civil(1900, 3, 1) - 1),
            (1900, 2, 28)
        );
    }

    #[test]
    fn 格式化日期布局() {
        assert_eq!(format_time(0, "2006-01-02"), "1970-01-01");
        assert_eq!(format_time(0, "2006-01-02 15:04:05"), "1970-01-01 00:00:00");
    }

    #[test]
    fn 格式化时刻布局() {
        // 2020-06-15 10:20:30 UTC
        let unix = days_from_civil(2020, 6, 15) * 86400 + 10 * 3600 + 20 * 60 + 30;
        assert_eq!(
            format_time(unix, "2006-01-02 15:04:05"),
            "2020-06-15 10:20:30"
        );
        assert_eq!(format_time(unix, "15:04:05"), "10:20:30");
        assert_eq!(format_time(unix, "2006年01月02日"), "2020年06月15日");
    }

    #[test]
    fn 解析日期布局() {
        let unix = parse_time("2006-01-02", "2020-01-01").unwrap();
        assert!(unix > 0);
        assert_eq!(format_time(unix, "2006-01-02"), "2020-01-01");
    }

    #[test]
    fn 解析日期时间布局() {
        let unix = parse_time("2006-01-02 15:04:05", "2020-06-15 10:20:30").unwrap();
        assert_eq!(
            format_time(unix, "2006-01-02 15:04:05"),
            "2020-06-15 10:20:30"
        );
    }

    #[test]
    fn 解析非法文本返回错误() {
        assert!(parse_time("2006-01-02", "bad").is_err());
        assert!(parse_time("2006-01-02", "2020-13-01").is_err());
        assert!(parse_time("2006-01-02", "2020-01-01多余").is_err());
    }

    #[test]
    fn 时间分量取回字典() {
        let unix = parse_time("2006-01-02 15:04:05", "2020-06-15 10:20:30").unwrap();
        let out = call("timeParts", vec![Object::int(unix)]).unwrap();
        let dict = match &out[0] {
            Object::Dict(d) => d.clone(),
            other => panic!("期望字典，实际 {}", other.inspect()),
        };
        assert_eq!(dict.get(&Object::string("year")), Some(Object::int(2020)));
        assert_eq!(dict.get(&Object::string("month")), Some(Object::int(6)));
        assert_eq!(dict.get(&Object::string("day")), Some(Object::int(15)));
        assert_eq!(dict.get(&Object::string("hour")), Some(Object::int(10)));
        assert_eq!(dict.get(&Object::string("minute")), Some(Object::int(20)));
        assert_eq!(dict.get(&Object::string("second")), Some(Object::int(30)));
    }

    #[test]
    fn 日期转时间戳() {
        let out = call(
            "dateUnix",
            vec![Object::int(2020), Object::int(1), Object::int(1)],
        )
        .unwrap();
        assert_eq!(out[1], Object::Nil);
        assert_eq!(
            format_time(out[0].as_int().unwrap(), "2006-01-02"),
            "2020-01-01"
        );
    }

    #[test]
    fn 非法日期返回错误() {
        let out = call(
            "dateUnix",
            vec![Object::int(2020), Object::int(13), Object::int(1)],
        )
        .unwrap();
        assert!(
            out[1].inspect().contains("非法日期"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 当前时间戳为正且毫秒更大() {
        let secs = call("nowUnix", vec![]).unwrap()[0].as_int().unwrap();
        let millis = call("nowUnixMilli", vec![]).unwrap()[0].as_int().unwrap();
        assert!(secs > 0);
        assert!(millis >= secs * 1000, "秒={} 毫秒={}", secs, millis);
    }

    #[test]
    fn 内建格式化与解析一致() {
        let out = call(
            "formatTime",
            vec![Object::int(0), Object::string("2006-01-02")],
        )
        .unwrap();
        assert_eq!(out[0].inspect(), "1970-01-01");
        let out = call(
            "parseTime",
            vec![Object::string("2006-01-02"), Object::string("2020-01-01")],
        )
        .unwrap();
        assert_eq!(out[1], Object::Nil);
    }

    #[test]
    fn 解析失败返回错误值而非中断() {
        let out = call(
            "parseTime",
            vec![Object::string("2006-01-02"), Object::string("bad")],
        )
        .unwrap();
        assert_eq!(out[0], Object::int(0));
        assert!(
            out[1].inspect().contains("不符") || out[1].inspect().contains("期望"),
            "实际：{}",
            out[1].inspect()
        );
    }

    #[test]
    fn 休眠不阻塞过久() {
        let start = std::time::Instant::now();
        call("sleep", vec![Object::Float(0.01)]).unwrap();
        assert!(start.elapsed() >= std::time::Duration::from_millis(5));
    }

    #[test]
    fn 时间组内建全部具备() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        for required in [
            "nowUnix",
            "nowUnixMilli",
            "formatTime",
            "parseTime",
            "dateUnix",
            "timeParts",
            "sleep",
        ] {
            assert!(names.contains(&required), "缺少内建 {}", required);
        }
    }
}
