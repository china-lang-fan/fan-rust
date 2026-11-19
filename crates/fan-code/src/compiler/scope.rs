//! 编译期的词法作用域与局部槽位分配。
//!
//! 每个作用域记录「名字 → 槽位」，由内向外查找以支持遮蔽与外层引用。M1 阶段
//! 槽位在函数内**只增不复用**：退出内层作用域不回收槽位，确保块内对块外变量
//! 的赋值始终解析到原始槽位，不会因游标回退而与外层变量撞槽。槽位紧凑化
//! （生命周期分析后复用）留待优化里程碑。

#[derive(Debug, Clone)]
struct Scope {
    names: Vec<(String, u16)>,
}

impl Scope {
    fn new() -> Self {
        Scope { names: Vec::new() }
    }
}

#[derive(Debug, Default)]
pub struct ScopeTracker {
    scopes: Vec<Scope>,
    /// 已分配的最大槽位 +1，即下一个可用槽位（单调递增）。
    next_slot: u16,
}

impl ScopeTracker {
    pub fn new() -> Self {
        ScopeTracker {
            scopes: vec![Scope::new()],
            next_slot: 0,
        }
    }

    pub fn push(&mut self) {
        self.scopes.push(Scope::new());
    }

    /// 退出作用域。槽位不回收，仅移除名字的可见性。
    pub fn pop(&mut self) {
        self.scopes.pop().expect("作用域栈非空");
    }

    /// 在当前作用域声明一个新名字，分配下一个槽位。
    pub fn declare(&mut self, name: &str) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        self.scopes
            .last_mut()
            .expect("至少有全局作用域")
            .names
            .push((name.to_string(), slot));
        slot
    }

    /// 把名字固定到指定槽位（用于函数参数的前置槽位）。
    pub fn declare_at(&mut self, name: &str, slot: u16) {
        if slot >= self.next_slot {
            self.next_slot = slot + 1;
        }
        self.scopes
            .last_mut()
            .expect("至少有全局作用域")
            .names
            .push((name.to_string(), slot));
    }

    /// 由内向外查找名字对应的槽位，未找到返回 None（调用方按全局/原生处理）。
    pub fn resolve(&self, name: &str) -> Option<u16> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, slot)) = scope.names.iter().rev().find(|(n, _)| n == name) {
                return Some(*slot);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 嵌套作用域外层名字仍可解析() {
        let mut tracker = ScopeTracker::new();
        let a = tracker.declare("甲");
        tracker.push();
        let b = tracker.declare("乙");
        assert_eq!(tracker.resolve("甲"), Some(a));
        assert_eq!(tracker.resolve("乙"), Some(b));
        tracker.pop();
        assert_eq!(tracker.resolve("乙"), None);
        assert_eq!(tracker.resolve("甲"), Some(a));
        // 退出后新声明拿到新槽位，不复用内层槽位。
        let c = tracker.declare("丙");
        assert_ne!(c, b);
    }

    #[test]
    fn 参数固定槽位后普通声明顺延() {
        let mut tracker = ScopeTracker::new();
        tracker.declare_at("参数甲", 0);
        tracker.declare_at("参数乙", 1);
        let local = tracker.declare("局部");
        assert_eq!(local, 2);
        assert_eq!(tracker.resolve("参数甲"), Some(0));
    }

    #[test]
    fn 内层遮蔽外层时内外槽位不同() {
        let mut tracker = ScopeTracker::new();
        let outer = tracker.declare("x");
        tracker.push();
        let inner = tracker.declare("x");
        assert_ne!(outer, inner);
        assert_eq!(tracker.resolve("x"), Some(inner));
        tracker.pop();
        assert_eq!(tracker.resolve("x"), Some(outer));
    }
}
