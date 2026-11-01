# fan-rust

凡语言的 Rust 实现：词法分析、语法分析、求值器、对象系统与命令行工具。

它是凡语言的两个实现之一，与 Go 版 [`china-lang-fan/fan`](https://github.com/china-lang-fan/fan)
共用同一份标准库 [`china-lang-fan/fan-sdk`](https://github.com/china-lang-fan/fan-sdk)。

## 快速开始

```bash
cargo build --release

# 编译产物叫 fan-cli，语言里的命令名是 fan
./target/release/fan-cli 运行 脚本.凡      # 执行脚本
./target/release/fan-cli 测试 ../fan-sdk   # 跑标准库测试
./target/release/fan-cli 版本
./target/release/fan-cli 帮助
```

`导入 "编码"` 这类模块引用，按以下顺序查找：脚本所在目录 → 环境变量
`FAN_SDK` → 可执行文件旁 → 系统安装位置。标准库不在本仓库内，本机开发时
用 `FAN_SDK` 指向它，或把 `fan-sdk` 放在本仓库的同级目录（测试会自动向上找）。

## 仓库结构

按依赖分层，单向依赖：`ast → object → native/eval → cli`。

| crate | 职责 |
| --- | --- |
| `fan-ast` | 抽象语法树节点与类型定义 |
| `fan-lexer` | 词法分析器与记号定义 |
| `fan-parser` | 语法分析器（Pratt 解析） |
| `fan-object` | 对象系统、运行环境、内建函数表 |
| `fan-eval` | 求值器（语句、表达式、模型、异常） |
| `fan-native` | 原生函数库，**只依赖标准库** |
| `fan-testrun` | `*_测试.凡` 的收集与运行 |
| `fan-cli` | 命令行入口 |
| `fan-ext-*` | 扩展库（依赖第三方库，编译为动态库按需加载） |

## 标签

凡代码用 `@标签` 标注声明。当前内置一种。

### `@内建("名字")`

把函数绑定到原生实现：绑定后调用该函数转调原生实现，函数体不再执行。

```
@内建("print")
公开 函数 打印(值...)
    返回 空
结束
```

名字统一用英文，可写成 `模块名.函数名`：

| 写法 | 含义 |
| --- | --- |
| `@内建("print")` | 只取系统已注册的（`core` 组）内建 |
| `@内建("json.jsonEncode")` | 按模块名加载扩展库（找不到则回落内置默认实现），再取其中的函数 |

模块名即动态库名（`json` → `libfan_ext_json`）。模块归属由凡代码显式声明，
解释器不维护「函数名 → 模块名」的反查表，因此新增扩展内建无需改动解释器。

## 测试

CI 与本地用同一套四道门禁：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p fan-cli && ./target/debug/fan-cli 测试 "$FAN_SDK"
```

前两道管风格与静态检查，第三道是 Rust 单测，第四道跑**凡语言写的**
标准库测试（`fan-sdk` 中的 `*_测试.凡`）。两道测试相辅相成：单测覆盖
解释器内部，标准库测试从语言使用者的角度验收。

## 扩展库机制

解释器本体保持**零第三方依赖**。正则、JSON、HTTP 这类依赖成熟第三方库
的能力，放在独立的 `fan-ext-*` crate 中，编译为动态库（cdylib），凡代码
真正用到时才 `dlopen` 加载——用 `@内建("模块名.函数名")` 引用即触发，
详见上面的「标签」一节。

- **找不到扩展库时自动回落**到 `fan-native` 内置的纯标准库实现，因此不
  部署任何扩展库，语言也能运行（能力受限，例如 HTTP 只支持明文）。
- 扩展库须导出两个符号：`fan_ext_abi_version()`（ABI 版本校验）与
  `fan_ext_register(rt)`（注册内建）。
- 扩展库必须与解释器链接**同一版本**的 `fan-object`，否则 `Runtime` 内存
  布局可能不一致——ABI 版本号用于拦截这种情况。
- 扩展库会**替换**回落实现，故对外行为必须与回落实现逐字一致，否则部署
  扩展库就会静默改变程序行为。各扩展库的测试都包含与默认实现的对比。

### 新增一个扩展库

1. 新建 `crates/fan-ext-xxx`，`crate-type = ["cdylib", "rlib"]`，依赖 `fan-object`。
2. 实现 `specs()`，返回 `NativeSpec::ext(内建名, 元数, 模块名, 闭包)`；
   导出上面两个符号。
3. 在根 `Cargo.toml` 的 `members` 与 `workspace.dependencies` 中注册。
4. 在 `fan-native` 的 `dev-dependencies` 加一行——这样 `cargo test` 会先
   产出动态库，端到端用例才能 `dlopen` 到它（运行期并不依赖）。
5. 模块名用 ASCII 英文并与库名一致（`json` → `libfan_ext_json`）。
   `lib_name` 里的「中文模块名 → 库名」映射仅为兼容既有写法保留。
6. 补两组测试：本库的行为单测，以及与默认实现逐项对比的端到端测试。

单测不要依赖外网或已构建出的扩展库：前者不稳定，后者会让结果取决于构建
产物是否存在（可用 `ExtRegistry::without_library()` 显式关闭扩展库加载）。

## 持续集成

`.github/workflows/ci.yml`：push 与 PR 触发，检出本仓库与标准库后依次跑
上面四道门禁。

## 许可

MIT，见 [LICENSE](LICENSE)。
