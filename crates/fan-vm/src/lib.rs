//! 凡语言栈式字节码虚拟机。
//!
//! 输入是 fan-code 编译产出的 [`ModuleChunk`]，本 crate 不依赖 parser/AST，只做
//! 取指-译码-执行。运行时值复用 fan-object 的 `Object`，原生内建与 FFI 在后续
//! 里程碑通过 `Runtime` 的原生表接入，VM 不重复实现宿主能力。
//!
//! ```text
//! fan-code（编译）→ ModuleChunk → fan-vm（本 crate，执行）
//! ```

mod error;
mod frame;
mod value;
mod vm;

pub use error::{VmError, VmResult};
pub use vm::Vm;

#[cfg(test)]
mod tests {
    use super::*;
    use fan_code::chunk::{ChunkBuilder, ModuleChunk};
    use fan_code::opcode::Opcode;
    use fan_code::ConstValue;
    use fan_object::Object;

    fn entry_with(builder: ChunkBuilder) -> ModuleChunk {
        ModuleChunk {
            entry: builder.build(),
            functions: Vec::new(),
            exports: Vec::new(),
            imports: Vec::new(),
        }
    }

    #[test]
    fn 常量压栈并停机返回该值() {
        let mut b = ChunkBuilder::new();
        let i = b.add_constant(ConstValue::Int(42));
        b.emit(Opcode::Const(i));
        b.emit(Opcode::Halt);
        let module = entry_with(b);

        let mut vm = Vm::new(&module);
        let result = vm.run().unwrap();
        assert_eq!(result, Object::Int(42));
    }

    #[test]
    fn 局部槽位存取往返() {
        let mut b = ChunkBuilder::new();
        let v = b.add_constant(ConstValue::Str("凡".to_string()));
        b.emit(Opcode::Const(v));
        b.emit(Opcode::StoreLocal(0));
        b.emit(Opcode::LoadLocal(0));
        b.emit(Opcode::Halt);
        let module = entry_with(b);

        let mut vm = Vm::new(&module);
        let result = vm.run().unwrap();
        assert_eq!(result, Object::string("凡"));
    }

    #[test]
    fn 条件跳转实现分支选择() {
        // 常量真 → 跳过“假分支压 0”，执行真分支压 1。
        let mut b = ChunkBuilder::new();
        let flag = b.add_constant(ConstValue::Bool(true));
        let one = b.add_constant(ConstValue::Int(1));
        let zero = b.add_constant(ConstValue::Int(0));
        b.emit(Opcode::Const(flag));
        let jump = b.emit(Opcode::JumpIfFalse(0));
        b.emit(Opcode::Pop);
        b.emit(Opcode::Const(one));
        let end = b.emit(Opcode::Jump(0));
        let false_target = b.code_len();
        b.emit(Opcode::Pop);
        b.emit(Opcode::Const(zero));
        let after = b.code_len();
        b.patch_jump(jump);
        b.patch_jump(end);
        let _ = (false_target, after);
        b.emit(Opcode::Halt);
        let module = entry_with(b);

        let mut vm = Vm::new(&module);
        assert_eq!(vm.run().unwrap(), Object::Int(1));
    }

    #[test]
    fn 未实现指令显式报错() {
        let mut b = ChunkBuilder::new();
        b.emit(Opcode::Throw);
        let module = entry_with(b);
        let mut vm = Vm::new(&module);
        let err = vm.run().unwrap_err();
        assert!(err.message.contains("尚未在 VM 中实现"));
    }

    #[test]
    fn 指令流缺少停机会报错() {
        let mut b = ChunkBuilder::new();
        let i = b.add_constant(ConstValue::Int(7));
        b.emit(Opcode::Const(i));
        // 故意不写 Halt。
        let module = entry_with(b);
        let mut vm = Vm::new(&module);
        assert!(vm.run().is_err());
    }
}
