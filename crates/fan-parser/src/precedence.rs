//! 运算符优先级。
//!
//! 与 Go 版 `precedence` 常量一一对应，数值越大结合越紧。

use crate::Precedence;
use fan_lexer::TokenType;

/// 优先级层级（低 → 高）。
pub const LOWEST: Precedence = 1;
pub const PREC_TERNARY: Precedence = 2;
pub const PREC_OR: Precedence = 3;
pub const PREC_AND: Precedence = 4;
pub const PREC_EQUALS: Precedence = 5;
pub const PREC_BIT_OR: Precedence = 6;
pub const PREC_BIT_XOR: Precedence = 7;
pub const PREC_BIT_AND: Precedence = 8;
pub const PREC_COMPARE: Precedence = 9;
pub const PREC_SHIFT: Precedence = 10;
pub const PREC_SUM: Precedence = 11;
pub const PREC_PRODUCT: Precedence = 12;
pub const PREC_PREFIX: Precedence = 13;
pub const PREC_POSTFIX: Precedence = 14;

/// 查询记号的中缀优先级。非中缀记号返回 `LOWEST`。
pub fn precedence_of(t: TokenType) -> Precedence {
    match t {
        TokenType::Quest => PREC_TERNARY,
        TokenType::Or => PREC_OR,
        TokenType::And => PREC_AND,
        TokenType::BOr => PREC_BIT_OR,
        TokenType::BXor => PREC_BIT_XOR,
        TokenType::BAnd => PREC_BIT_AND,
        TokenType::Shl | TokenType::Shr => PREC_SHIFT,
        TokenType::Eq | TokenType::Neq => PREC_EQUALS,
        TokenType::Lt | TokenType::Lte | TokenType::Gt | TokenType::Gte => PREC_COMPARE,
        TokenType::Is | TokenType::As => PREC_COMPARE,
        TokenType::Member | TokenType::Dot => PREC_POSTFIX,
        TokenType::Plus | TokenType::Minus => PREC_SUM,
        TokenType::Star | TokenType::Slash | TokenType::Percent => PREC_PRODUCT,
        _ => LOWEST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 优先级层级由低到高递增() {
        // 常量之间的比较用编译期断言：写成运行期 assert 会被优化掉
        const _: () = assert!(LOWEST < PREC_TERNARY);
        const _: () = assert!(PREC_TERNARY < PREC_OR);
        const _: () = assert!(PREC_OR < PREC_AND);
        const _: () = assert!(PREC_AND < PREC_EQUALS);
        const _: () = assert!(PREC_EQUALS < PREC_BIT_OR);
        const _: () = assert!(PREC_BIT_OR < PREC_BIT_XOR);
        const _: () = assert!(PREC_BIT_XOR < PREC_BIT_AND);
        const _: () = assert!(PREC_BIT_AND < PREC_COMPARE);
        const _: () = assert!(PREC_COMPARE < PREC_SHIFT);
        const _: () = assert!(PREC_SHIFT < PREC_SUM);
        const _: () = assert!(PREC_SUM < PREC_PRODUCT);
        const _: () = assert!(PREC_PRODUCT < PREC_PREFIX);
        const _: () = assert!(PREC_PREFIX < PREC_POSTFIX);
    }

    #[test]
    fn 乘除优先级高于加减() {
        assert!(precedence_of(TokenType::Star) > precedence_of(TokenType::Plus));
        assert!(precedence_of(TokenType::Slash) > precedence_of(TokenType::Minus));
        assert!(precedence_of(TokenType::Percent) > precedence_of(TokenType::Plus));
    }

    #[test]
    fn 比较与等于优先级关系() {
        assert!(precedence_of(TokenType::Lt) > precedence_of(TokenType::Eq));
        assert_eq!(precedence_of(TokenType::Eq), precedence_of(TokenType::Neq));
    }

    #[test]
    fn 位运算优先级介于等于与比较之间() {
        assert!(precedence_of(TokenType::BOr) > precedence_of(TokenType::Eq));
        assert!(precedence_of(TokenType::BOr) < precedence_of(TokenType::Lt));
        assert_eq!(precedence_of(TokenType::Shl), precedence_of(TokenType::Shr));
    }

    #[test]
    fn 是转为与比较同级() {
        assert_eq!(precedence_of(TokenType::Is), PREC_COMPARE);
        assert_eq!(precedence_of(TokenType::As), PREC_COMPARE);
    }

    #[test]
    fn 成员与点号是后缀级() {
        assert_eq!(precedence_of(TokenType::Member), PREC_POSTFIX);
        assert_eq!(precedence_of(TokenType::Dot), PREC_POSTFIX);
    }

    #[test]
    fn 非运算符记号优先级最低() {
        assert_eq!(precedence_of(TokenType::Ident), LOWEST);
        assert_eq!(precedence_of(TokenType::Int), LOWEST);
        assert_eq!(precedence_of(TokenType::LParen), LOWEST);
    }
}
