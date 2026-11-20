//! 字节码产物的二进制读写（零第三方依赖）。
//!
//! 编码采用小端序与长度前缀：所有可变长集合先写 `u32` 长度再写元素，字符串按
//! UTF-8 编码。文件整体布局为 [`MAGIC`] + 版本号 + [`ModuleChunk`]。版本不符或
//! 魔数错误时拒绝加载，由上层回退到源码重新编译。

use crate::chunk::{
    Arity, Chunk, ConstValue, FunctionChunk, ModuleChunk, UpvalueDesc, BYTECODE_VERSION, MAGIC,
};

/// 序列化错误。
pub type CodeResult<T> = Result<T, String>;

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }

    fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    fn string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
    }

    fn bytes(&mut self, data: &[u8]) {
        self.u32(data.len() as u32);
        self.buf.extend_from_slice(data);
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> CodeResult<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err("字节码数据意外结束".to_string());
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> CodeResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> CodeResult<u16> {
        let mut raw = [0u8; 2];
        raw.copy_from_slice(self.take(2)?);
        Ok(u16::from_le_bytes(raw))
    }

    fn u32(&mut self) -> CodeResult<u32> {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(raw))
    }

    fn u64(&mut self) -> CodeResult<u64> {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(raw))
    }

    fn i64(&mut self) -> CodeResult<i64> {
        Ok(self.u64()? as i64)
    }

    fn f64(&mut self) -> CodeResult<f64> {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(self.take(8)?);
        Ok(f64::from_le_bytes(raw))
    }

    fn bool(&mut self) -> CodeResult<bool> {
        Ok(self.u8()? != 0)
    }

    fn string(&mut self) -> CodeResult<String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(|s| s.to_string())
            .map_err(|e| format!("字符串编码非法：{e}"))
    }

    fn bytes(&mut self) -> CodeResult<Vec<u8>> {
        let len = self.u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }
}

impl ConstValue {
    fn write(&self, w: &mut Writer) {
        match self {
            ConstValue::Nil => w.u8(0),
            ConstValue::Bool(b) => {
                w.u8(1);
                w.bool(*b);
            }
            ConstValue::Int(i) => {
                w.u8(2);
                w.i64(*i);
            }
            ConstValue::Float(f) => {
                w.u8(3);
                w.f64(*f);
            }
            ConstValue::Byte(b) => {
                w.u8(4);
                w.u8(*b);
            }
            ConstValue::Char(c) => {
                w.u8(5);
                w.u32(*c as u32);
            }
            ConstValue::Str(s) => {
                w.u8(6);
                w.string(s);
            }
        }
    }

    fn read(r: &mut Reader) -> CodeResult<ConstValue> {
        let tag = r.u8()?;
        Ok(match tag {
            0 => ConstValue::Nil,
            1 => ConstValue::Bool(r.bool()?),
            2 => ConstValue::Int(r.i64()?),
            3 => ConstValue::Float(r.f64()?),
            4 => ConstValue::Byte(r.u8()?),
            5 => {
                let code = r.u32()?;
                let c = char::from_u32(code).ok_or_else(|| format!("非法字符码点：{code}"))?;
                ConstValue::Char(c)
            }
            6 => ConstValue::Str(r.string()?),
            other => return Err(format!("未知常量类型标记：{other}")),
        })
    }
}

impl Chunk {
    fn write(&self, w: &mut Writer) {
        w.bytes(&self.code);
        w.u32(self.constants.len() as u32);
        for c in &self.constants {
            c.write(w);
        }
        w.u32(self.lines.len() as u32);
        for line in &self.lines {
            w.u32(*line);
        }
    }

    fn read(r: &mut Reader) -> CodeResult<Chunk> {
        let code = r.bytes()?;
        let const_len = r.u32()? as usize;
        let mut constants = Vec::with_capacity(const_len);
        for _ in 0..const_len {
            constants.push(ConstValue::read(r)?);
        }
        let line_len = r.u32()? as usize;
        let mut lines = Vec::with_capacity(line_len);
        for _ in 0..line_len {
            lines.push(r.u32()?);
        }
        Ok(Chunk {
            code,
            constants,
            lines,
        })
    }
}

impl Arity {
    fn write(&self, w: &mut Writer) {
        w.u64(self.fixed as u64);
        w.bool(self.variadic);
        w.u64(self.optional as u64);
    }

    fn read(r: &mut Reader) -> CodeResult<Arity> {
        Ok(Arity {
            fixed: r.u64()? as usize,
            variadic: r.bool()?,
            optional: r.u64()? as usize,
        })
    }
}

impl UpvalueDesc {
    fn write(&self, w: &mut Writer) {
        w.string(&self.name);
        w.bool(self.from_local);
        w.u16(self.index);
    }

    fn read(r: &mut Reader) -> CodeResult<UpvalueDesc> {
        Ok(UpvalueDesc {
            name: r.string()?,
            from_local: r.bool()?,
            index: r.u16()?,
        })
    }
}

impl FunctionChunk {
    fn write(&self, w: &mut Writer) {
        w.string(&self.name);
        self.arity.write(w);
        w.u32(self.params.len() as u32);
        for p in &self.params {
            w.string(p);
        }
        self.body.write(w);
        w.u32(self.upvalues.len() as u32);
        for u in &self.upvalues {
            u.write(w);
        }
    }

    fn read(r: &mut Reader) -> CodeResult<FunctionChunk> {
        let name = r.string()?;
        let arity = Arity::read(r)?;
        let param_len = r.u32()? as usize;
        let mut params = Vec::with_capacity(param_len);
        for _ in 0..param_len {
            params.push(r.string()?);
        }
        let body = Chunk::read(r)?;
        let up_len = r.u32()? as usize;
        let mut upvalues = Vec::with_capacity(up_len);
        for _ in 0..up_len {
            upvalues.push(UpvalueDesc::read(r)?);
        }
        Ok(FunctionChunk {
            name,
            arity,
            params,
            body,
            upvalues,
        })
    }
}

impl ModuleChunk {
    fn write_body(&self, w: &mut Writer) {
        self.entry.write(w);
        w.u32(self.functions.len() as u32);
        for f in &self.functions {
            f.write(w);
        }
        w.u32(self.exports.len() as u32);
        for e in &self.exports {
            w.string(e);
        }
        w.u32(self.imports.len() as u32);
        for i in &self.imports {
            w.string(i);
        }
    }

    fn read_body(r: &mut Reader) -> CodeResult<ModuleChunk> {
        let entry = Chunk::read(r)?;
        let fn_len = r.u32()? as usize;
        let mut functions = Vec::with_capacity(fn_len);
        for _ in 0..fn_len {
            functions.push(FunctionChunk::read(r)?);
        }
        let export_len = r.u32()? as usize;
        let mut exports = Vec::with_capacity(export_len);
        for _ in 0..export_len {
            exports.push(r.string()?);
        }
        let import_len = r.u32()? as usize;
        let mut imports = Vec::with_capacity(import_len);
        for _ in 0..import_len {
            imports.push(r.string()?);
        }
        Ok(ModuleChunk {
            entry,
            functions,
            exports,
            imports,
        })
    }
}

/// 把模块块编码为完整产物字节（含魔数与版本头）。
pub fn encode_module(module: &ModuleChunk) -> Vec<u8> {
    let mut w = Writer::new();
    w.buf.extend_from_slice(&MAGIC);
    w.u16(BYTECODE_VERSION);
    module.write_body(&mut w);
    w.buf
}

/// 从产物字节解码模块块，校验魔数与版本。
pub fn decode_module(data: &[u8]) -> CodeResult<ModuleChunk> {
    let mut r = Reader::new(data);
    let magic = r.take(MAGIC.len())?;
    if magic != MAGIC {
        return Err("不是凡语言字节码产物（魔数不匹配）".to_string());
    }
    let version = r.u16()?;
    if version != BYTECODE_VERSION {
        return Err(format!(
            "字节码版本 {version} 与当前支持的 {BYTECODE_VERSION} 不匹配，请重新编译"
        ));
    }
    let module = ModuleChunk::read_body(&mut r)?;
    if r.pos != data.len() {
        return Err("字节码产物存在未消费的尾部数据".to_string());
    }
    Ok(module)
}
