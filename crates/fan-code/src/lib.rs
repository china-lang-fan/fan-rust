//! 凡语言字节码：编译期的指令契约、代码块数据、反汇编与产物序列化。
//!
//! 本 crate 处于「源码 → 字节码」一侧，只依赖 Rust 标准库：既不引用 AST，也
//! 不包含任何执行逻辑。运行期的取指-译码-执行由 `fan-vm` 负责，二者通过本
//! crate 定义的 [`ModuleChunk`] 通信。
//!
//! ```text
//! fan-ast → (fan-code 编译) → ModuleChunk → (fan-vm 执行)
//!                  └─ 序列化 → .fanc / .凡码（写入项目 target/）
//! ```

pub mod artifact;
pub mod chunk;
pub mod compiler;
pub mod disasm;
pub mod opcode;
pub mod serialize;

pub use artifact::{artifact_path, compiled_extension, is_fan_source};
pub use chunk::{
    Arity, Chunk, ChunkBuilder, ConstValue, FunctionChunk, ModuleChunk, UpvalueDesc,
    BYTECODE_VERSION, MAGIC, MANIFEST_MARKER,
};
pub use disasm::{disassemble, DisasmLine};
pub use opcode::Opcode;
pub use serialize::{decode_module, encode_module, CodeResult};

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_module() -> ModuleChunk {
        let mut entry = ChunkBuilder::new();
        entry.set_line(1);
        let idx = entry.add_constant(ConstValue::Int(42));
        entry.emit(Opcode::Const(idx));
        entry.emit(Opcode::Halt);

        let mut fn_body = ChunkBuilder::new();
        fn_body.set_line(3);
        let s = fn_body.add_constant(ConstValue::Str("凡".to_string()));
        fn_body.emit(Opcode::Const(s));
        fn_body.emit(Opcode::Return);

        ModuleChunk {
            entry: entry.build(),
            functions: vec![FunctionChunk {
                name: "加倍".to_string(),
                arity: Arity {
                    fixed: 1,
                    variadic: false,
                    optional: 0,
                },
                params: vec!["数".to_string()],
                body: fn_body.build(),
                upvalues: vec![UpvalueDesc {
                    from_local: true,
                    index: 0,
                }],
            }],
            exports: vec!["加倍".to_string()],
            imports: vec!["依赖/问候包/库/问候".to_string()],
        }
    }

    #[test]
    fn 序列化往返保持模块一致() {
        let module = sample_module();
        let bytes = encode_module(&module);
        assert_eq!(&bytes[0..4], &MAGIC);
        let restored = decode_module(&bytes).expect("解码应成功");
        assert_eq!(module, restored);
    }

    #[test]
    fn 拒绝错误魔数与坏版本() {
        let bytes = encode_module(&sample_module());
        assert!(decode_module(&bytes[1..]).is_err());
        let mut bad_version = bytes.clone();
        bad_version[4] = 0xFF;
        bad_version[5] = 0xFF;
        assert!(decode_module(&bad_version).is_err());
    }

    #[test]
    fn 反汇编输出指令与常量() {
        let module = sample_module();
        let lines = disassemble(&module.entry).expect("反汇编应成功");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].offset, 0);
        assert_eq!(lines[0].line, 1);
        assert!(lines[0].text.contains("CONST"));
        assert!(lines[0].text.contains("Int(42)"));
        assert!(lines[1].text.contains("Halt"));
    }

    #[test]
    fn 常量池去重并返回同一索引() {
        let mut b = ChunkBuilder::new();
        let a = b.add_constant(ConstValue::Str("x".to_string()));
        let c = b.add_constant(ConstValue::Str("x".to_string()));
        assert_eq!(a, c);
        assert_eq!(b.build().constants.len(), 1);
    }
}
