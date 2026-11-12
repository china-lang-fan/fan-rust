# fan-rust

凡语言的 Rust 实现：词法分析、语法分析、求值器、对象系统与命令行工具。

它是凡语言的两个实现之一，与 Go 版 [`china-lang-fan/fan`](https://github.com/china-lang-fan/fan)
共用同一份标准库 [`china-lang-fan/fan-sdk`](https://github.com/china-lang-fan/fan-sdk)。

## 快速开始

```bash
cargo build --release

# crate 包名是 fan-cli，产出的可执行文件（语言命令）叫 fan
./target/release/fan 运行 脚本.凡      # 执行脚本
./target/release/fan 测试 ../fan-sdk   # 跑标准库测试
./target/release/fan 版本
./target/release/fan 帮助
```

`导入 "编码"` 这类模块引用，按以下顺序查找：脚本所在目录 → 环境变量
`FAN_SDK` → 可执行文件旁 → 系统安装位置。标准库不在本仓库内，本机开发时
用 `FAN_SDK` 指向它，或把 `fan-sdk` 放在本仓库的同级目录（测试会自动向上找）。

## 示例

[`examples/`](./examples) 收集可直接运行的凡语言脚本，覆盖变量与运算、
控制流、函数、多返回值、模型、错误处理、字典/数组、标签与模块导入等：

```bash
./target/debug/fan 运行 examples/函数.fan
./target/debug/fan 运行 examples/模型.fan
./target/debug/fan 运行 examples/模块.fan      # 依赖 examples/modules 子目录
```

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
| `fan-cli` | 命令行入口（crate 包名；产出的可执行文件为 `fan`） |
| `fan-ext-*` | 扩展库（依赖第三方库，编译为动态库按需加载） |

## 标签与本地绑定

凡代码用 `@标签` 标注声明。标签框架是通用扩展点（如测试框架用 `@测试`
识别测试函数、将来可做切面），处理器按需注册。

### `定义 本地("名字") 函数 ...`（推荐）

把函数绑定到原生实现。调用时**优先转调原生实现**；若找不到原生实现，则
**静默回退执行函数体**，不产生运行时错误——函数体既是可移植兜底，也可
主动 `抛出` 异常声明「此能力必须由本地实现」。

```
定义 本地("print") 函数 打印(值...)
    返回 空
结束
```

硬规则：`本地` 后必须紧跟括号，括号内只能是单个字符串字面量（不支持插值
或变量）；只能用于顶层函数，不能用于类型方法；函数体必需（作为兜底）。

名字统一用英文，可写成 `模块名.函数名`：

| 写法 | 含义 |
| --- | --- |
| `定义 本地("print") 函数 ...` | 只取系统已注册的（`core` 组）内建 |
| `定义 本地("json.jsonEncode") 函数 ...` | 按模块名加载扩展库（找不到则回落内置默认实现），再取其中的函数 |

以下三种「找不到」都会静默回退函数体：core 未注册该名、扩展库加载失败、
模块中没有该函数。LSP 会对这三种情况给出 Warning 级静态提示（不阻断运行）。

命名契约（core 与 ext 两组）：

- **core 组**（解释器内置、零第三方依赖，见 `fan-native/src/core/`）一律用**裸名**，
  如 `本地("print")`、`本地("sqrt")`、`本地("base64Encode")`。
- **ext 组**（按需 `dlopen` 的动态库，见 `fan-ext-*`）一律用 `模块.函数`，
  如 `本地("json.jsonEncode")`、`本地("regex.regexMatch")`、`本地("tag.tagRegisterHandler")`。
- 模块段到动态库的映射在 `fan-native` 的 `lib_name` 中显式登记（含中文别名
  `正则→regex`、`网络→http`、`标签→tag`）；每个 ext 模块都有等价的内置默认回落，
  未部署动态库也能运行。新增 ext 库时需同步补 `lib_name` 与默认回落。

模块名即动态库名（`json` → `libfan_ext_json`）。模块归属由凡代码显式声明，
解释器不维护「函数名 → 模块名」的反查表，因此新增扩展内建无需改动解释器。

## 测试

CI 与本地用同一套四道门禁：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p fan-cli && ./target/debug/fan 测试 "$FAN_SDK"
```

前两道管风格与静态检查，第三道是 Rust 单测，第四道跑**凡语言写的**
标准库测试（`fan-sdk` 中的 `*_测试.凡`）。两道测试相辅相成：单测覆盖
解释器内部，标准库测试从语言使用者的角度验收。

## 扩展库机制

解释器本体保持**零第三方依赖**。正则、JSON、HTTP 这类依赖成熟第三方库
的能力，放在独立的 `fan-ext-*` crate 中，编译为动态库（cdylib），凡代码
真正用到时才 `dlopen` 加载——用 `定义 本地("模块名.函数名") 函数 ...`
引用即触发，详见上面的「标签与本地绑定」一节。

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

## 持续集成与发布

`.github/workflows/ci.yml`：push 与 PR 触发，检出本仓库与标准库后依次跑
上面四道门禁。

`.github/workflows/release.yml`：打 `v` 开头的 tag 触发。先跑同一套门禁，
通过后为各平台构建并打包，安装包挂到对应的 Release。

### 发版

版本号只认 `Cargo.toml` 的 `version`（`fan 版本` 读的就是它）。tag 必须与
它一致，不一致会直接在门禁里报错，以免出现「包叫 0.2.0、`fan 版本` 却报
0.1.0」：

```bash
# 1. 改 Cargo.toml 的 version 并提交
# 2. 打同版本的 tag
git tag v0.2.0
git push origin v0.2.0
```

### 安装包

每个平台的安装包都是「可执行文件 + 标准库」：

```
fan-0.2.0-x86_64-unknown-linux-gnu/
├── fan            # 可执行（Windows 下为 fan.exe）
└── sdk/           # 标准库，导入时自动查找
```

解压即用，无需再设 `FAN_SDK`——标准库就在可执行文件旁边。

| 平台 | 产物 |
| --- | --- |
| Linux x86_64 | `fan-<版本>-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `fan-<版本>-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `fan-<版本>-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `fan-<版本>-x86_64-pc-windows-msvc.zip` |

macOS 的二进制未签名，首次运行若被 Gatekeeper 拦下，执行
`xattr -d com.apple.quarantine fan` 即可。

## 许可

MIT，见 [LICENSE](LICENSE)。
