# Linux amd64 原生构建契约与 ISA 证据边界

本页只核实已支持的 `x86_64-unknown-linux-gnu` 原生发行路径，不新增平台、不擅自选择
x86-64-v3，也不把某台机器成功运行外推为所有 amd64 机器通过。

## 原生与浏览器分开

`.github/scripts/release-native.py` 以 `--locked --offline --release --target
x86_64-unknown-linux-gnu` 构建 `storage-server`／`storage-rescue`，交付程序为 `zxs`／
`zxs-rescue`。浏览器包由独立 `wasm32-unknown-unknown` 构建及 Web manifest 验证；浏览器
WASM 特性或成功不能充当 Linux ISA 证据。归档内容、启动路径及系统要求见
[安装模板](../templates/release-install.md)与[发行清单](release-checklist.md)。

仓库 `.cargo/config.toml` 没有设置全局 `target-cpu`／`target-feature`。官方 Rust 1.99.0 对
该 target 的实际默认查询给出 CPU `x86-64`，默认 features 为 `fxsr`、`sse`、`sse2`。
此查询不应用 Cargo flags，不是对最终二进制或所有 C／汇编对象的最低 ISA 认证。

## 可重现、安全的 BUILD 收据

每次原生发行构建除既有 compiler/version、源码、配置／环境摘要及两二进制哈希外，再记录：

- `native-rust-target-default-cpu`：同一所选 rustc 的 `--print target-cpus --target ...` 默认值。
- `native-rust-target-default-cfg`：同一 rustc `--print cfg --target ...` 的 `target_*` 项；
  不把默认 `debug_assertions` 等当成实际 release 编译参数。
- `native-build-overrides-present`、`native-rust-flag-overrides-present`、
  `native-c-flag-overrides-present`：只记录覆盖变量是否存在（包括显式空值），不披露
  任意环境变量的名称、私有路径、宏值或 flag 文本。完整选择继续由原环境摘要绑定。
- `native-isa-attestation=not-attested; target-default-query-only; excludes-effective-Rust-C-asm-flags`。

默认查询和覆盖存在性不能证明实际有效参数；即使覆盖为 false，也不能代替构建脚本／
编译器命令及目标机器验证。当前收据不声称逐 C／汇编对象参数已被记录。若查询缺失、
不唯一、target 不符或包含不可安全记录的形式，则拒绝生成成功收据，不回填猜测值。

## Rust／C／汇编的实际来源边界

| 来源 | 已核实的当前源码行为 | 不能据此推出 |
|---|---|---|
| `foundation/tooling/cc/src/flags.rs` | 将已知 Rust `target-feature` 映射为兼容 C flags；没有通用 `target-cpu` 映射；cc 最后还应用 CFLAGS 覆盖 | Rust CPU 选择必然等于 C 编译参数 |
| `foundation/cryptography/blake3/build.rs`、`src/platform.rs` | 分别构建 SSE／AVX2／AVX512 专用 archive；AVX512 flags 用于专用对象；运行时检测后分派，intrinsics 有函数级 target_feature | 链接了 AVX512 archive 就要求整个程序最低 AVX512／v3 |
| `foundation/cryptography/ring/build.rs`、`src/cpu/intel.rs` | x86_64 不自行全局升为高 ISA；CPUID 与 OS AVX 状态检测，具体算法按 feature token 选择 | 每个目标机器与运行分支已经实测 |
| `foundation/data-storage/libsqlite3-sys/build.rs` | bundled SQLite C 经 cc 构建，没有自行设置更高 x86 ISA | 外部 CFLAGS 或编译器默认值不能改变结果 |

现成本机 release build-script output 可见 BLAKE3 专用 archives 与 bundled sqlite3 静态链接，
所列 CFLAGS 覆盖为空；ring 的现成本机证据仅有 debug output。这些不能冒充正式发行产物
的逐对象编译实录。以上源码路径均相对 `src/`；精确源码由 BUILD 的 commit 绑定。

## 已有真实发行流程证据

[CI 41183041841](https://github.com/isarmg/zxs/actions/runs/41183041841) 的
[Chrome／归档验收 job 124485872867](https://github.com/isarmg/zxs/actions/runs/41183041841/job/124485872867)
绑定 `0787d77cf8fa074f6d268bbf05444e54cc07ca52`：实际调用 release-native，release 构建成功，
随后运行提取归档验收，日志记录 `BUILD.txt: OK`，产出 `installation-candidate-linux-web`。
该 job 证明此 Linux runner 上的构建／归档验收，不记录所有实际编译命令或证明最低 CPU／
最低 GNU libc。CI 安装候选也不等于已经发布的 GitHub Release 资产。新增收据字段不能
反向补到旧产物；须由后续实际原生发行构建生成并随对应二进制保存。

对该实际安装候选 artifact `13036011670` 进行只读核验：ZIP SHA256 为
`afec30d90efabcf6e61c9e939a06ddc96d775322a9bc079dce796af10d709d7c`，原 BUILD 的
两个可执行文件摘要均匹配。两程序的 ELF interpreter 都是 `/lib64/ld-linux-x86-64.so.2`，
直接 NEEDED 为 `libgcc_s.so.1`、`libm.so.6`、`libc.so.6`、`ld-linux-x86-64.so.2`；
直接 GNU libc 符号版本需求最高是 `GLIBC_2.34`。这是这两个确切产物的静态检查，不是
“所有 glibc 2.34 系统均受支持”的承诺，也没有执行部署。它们不是完全静态、无运行库依赖的
单文件。安装候选的 native／Web 归档与 `BUILD.txt`、`INSTALL.md`、`SHA256SUMS` 分开提供。


## 新收据字段的实际发行产物（2026-10-31）

[CI 41186409342](https://github.com/isarmg/zxs/actions/runs/41186409342) 完整 17 jobs 成功，
[安装候选 artifact 13037263126](https://github.com/isarmg/zxs/actions/runs/41186409342/artifacts/13037263126)
ZIP SHA256 为 `6c15320a9b2f2e942e16affa61928f1776f565ce3bdefcf3894c750b58aa0baf`。
实际 BUILD 绑定 `805cec7a5143e4966d47fffe66f5433055545454`、Rust 1.99.0、
`x86_64-unknown-linux-gnu`；默认查询 CPU 为 `x86-64`，feature 为 `fxsr`／`sse`／`sse2`，
三类 overrides-present 均为 false，并明确保留 `not-attested`／不包含实际 Rust/C/asm flags 的声明。
已实际读取归档并逐字节核对 native tar
`36ffe9cba159802f53fdbfb893e0c96e6473676945fc8c70b20dcf9fb0c7fb50`、zxs
`7992b344779789bb4269ce8c16ee991344127be651519118cbb1d017f9081bc3`、zxs-rescue
`fd19cbd8c637deed3f7b60e793ec4bbed1ec2750d2d4df68b55373b576a71be1` 与 BUILD 一致。
这是新字段的真实构建／归档证据，不升级为全 ISA 证明、任意发行版支持或实际部署承诺。
