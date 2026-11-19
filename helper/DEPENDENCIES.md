# 依赖与固定版本

[Cargo.toml](Cargo.toml) 在主程序中声明四个非开发的直接注册表依赖，另有一个本项目内部小库：

- `rusqlite = 0.43.0`（关闭默认功能，仅 `bundled`）：用上游的 SQLite 连接、参数绑定和事务，不调用外部 shell/数据库服务。小型 native 边界与它共享锁定的同一个 `libsqlite3-sys 0.40.0` 引擎，只读取源码 ID、编译选项与原 Unix VFS 路径上限。
- `httparse = 1.10.1`：只用成熟的 HTTP/1.1 请求头解析器；Web 自己限制方法、来源、请求长度、时限、操作和会话。没有新增传递普通依赖。
- `rustix = 1.1.5`（默认 `std`，显式 `fs`、`rand`、`pipe`、`process`、`termios`、`thread`）：安全的 Linux 文件描述符和文件系统观察；新池/卷 ID 只使用内核非阻塞 getrandom。本次仅启用同一锁定包的现有功能，没有增加依赖或改锁。
- `sha2 = 0.10.9`（不启用默认功能）：在内存中以 RustCrypto SHA-256 核验候选新登记完整前缀；不用于认证操作者或保护口令。

主程序的测试直接用 `linux-raw-sys 0.12.1` 核已知 Linux 文件系统类型。`tempfile = 3.27.0` 仅供测试，在专用临时父目录内独占创建私有子目录；它不是运行时组件。位置参数、字节流和输出使用标准库。支持的 Rust 工具链单独固定为 1.100.0。

本项目内部的 [helper-linux-evidence](linux-evidence/Cargo.toml) 是单独静审的 Linux LP64 小库，不依赖其他项目。它直接使用锁内的 `libc 0.2.190`、`linux-raw-sys 0.12.1` 和 `rustix 1.1.5`。主程序仍禁止 unsafe；小库的固定 Linux/POSIX 只读接口使用少量 unsafe：通过持有的原描述符读取 [FS_IOC_GETFSUUID](https://github.com/torvalds/linux/blob/master/include/uapi/linux/fs.h) 和 [name_to_handle_at](https://www.man7.org/linux/man-pages/man2/name_to_handle_at.2.html)；每个卷行的原生名称字节限额使用同目录 [fpathconf(_PC_NAME_MAX)](https://man7.org/linux/man-pages/man3/fpathconf.3.html)，不猜未知值。另有一个仅供独立诊断 CLI 的 [close_range](https://man7.org/linux/man-pages/man2/close_range.2.html) 固定调用，私有化描述符表并给所有非标准描述符加 close-on-exec，不关闭外部文件。隔离运行的极小 Linux 层还核验可等待的子进程信号策略，在本次 CLI 处理取消消息；子进程在启动之前用 setsid 建独立组，并仅对已核的 ELF 文件描述符使用 execveat。主程序没有 unsafe。没有任意 ioctl、路径重开或按句柄开文件的接口。libc 现在是小库的直接普通依赖，不是新增或升级了注册表包。

本工具的 13 项原有注册表记录取自 **cc 1.8.0** 官方发行包的 Cargo.lock，未改版本。新增 8 项按 **[sha2 0.10.9](https://crates.io/crates/sha2/0.10.9)** 官方发行包的 Cargo.lock 固定版本，逐个与 crates.io 原档案、登记校验值及其规范化 Cargo.toml 核对。锁内记录本工程选用的依赖，包括本工程的测试依赖及所需的构建依赖；不引 sha2 自身的测试、可选的汇编/OID，亦不引 digest 的测试数据。共用 cfg-if 和 libc 的原锁版本。这份锁不是在本工程新运行解析而得。跨平台传递记录不表示支持其他平台。

在独立目录使用标准 crates.io 源，或使用已备齐精确版本的本机可信缓存；不从父目录导入清单、源码、锁文件或构建产物。缓存不全时显式报告缺件，不替换包名或校验值。本次未捆绑或安装第三方二进制。已有[离线工具包配方](packaging/debian13-amd64/README.md)分别核上游工具、库、准确原包和对应源码；这些不是 Cargo 依赖。

## 已核对的 32 份发行源码包

`httparse` 的完整锁记录由 [crates.io 登记](https://github.com/rust-lang/crates.io-index/blob/master/ht/tp/httparse)及上游 axum 的 Cargo.lock 对读；原始发行包 SHA-256 为 `6dbf3de79e51f3d586ab4cb9d5c3e2c14aa28ed23d180cf89b4df0454a69cc87`，静态核对了包内许可；未执行其 build.rs、解析器或 Cargo。

本表对应本项目 [Cargo.lock](Cargo.lock)（文件 SHA-256：`beb3e6601f321fe68c34ec0535d1821ade078ca252cd6686fc0d62ff369dfbbd`）。逐个从 crates.io 取了锁定版本的 `.crate`，先核整个归档 SHA-256 与锁里该包的 `checksum` 相同，再静态读取归档中的 `Cargo.toml`、许可及署名材料。没有安装、执行或重新解析这些包。表内链接指向各版本的注册表页面；[Cargo 注册表规范](https://doc.rust-lang.org/cargo/reference/registry-index.html#download-endpoint)给出原始包的下载方式。

| 精确发行包 | 谁在锁里引用它 | 包声明的许可表达式 | 包内已核材料 |
| --- | --- | --- | --- |
| [cc 1.11.2](https://crates.io/crates/cc/1.11.2) | libsqlite3-sys 编译原 C | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [fallible-iterator 0.3.0](https://crates.io/crates/fallible-iterator/0.3.0) | rusqlite | `MIT/Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [fallible-streaming-iterator 0.1.9](https://crates.io/crates/fallible-streaming-iterator/0.1.9) | rusqlite | `MIT/Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [find-msvc-tools 0.1.15](https://crates.io/crates/find-msvc-tools/0.1.15) | cc | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [libsqlite3-sys 0.40.0](https://crates.io/crates/libsqlite3-sys/0.40.0) | rusqlite 与本工程 native 资格读取 | `MIT` | `LICENSE` |
| [pkg-config 0.3.34](https://crates.io/crates/pkg-config/0.3.34) | libsqlite3-sys 构建依赖；此处不向系统库回退 | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [rusqlite 0.43.0](https://crates.io/crates/rusqlite/0.43.0) | 主程序 | `MIT` | `LICENSE` |
| [shlex 2.0.1](https://crates.io/crates/shlex/2.0.1) | cc | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [smallvec 1.16.5](https://crates.io/crates/smallvec/1.16.5) | rusqlite | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [vcpkg 0.2.15](https://crates.io/crates/vcpkg/0.2.15) | libsqlite3-sys 构建依赖；不代表支持 Windows | `MIT/Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [httparse 1.10.1](https://crates.io/crates/httparse/1.10.1) | 主程序直接依赖；上游测试依赖不引入 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [bitflags 2.13.2](https://crates.io/crates/bitflags/2.13.2) | rustix | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [cfg-if 1.0.5](https://crates.io/crates/cfg-if/1.0.5) | getrandom、sha2 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [errno 0.3.14](https://crates.io/crates/errno/0.3.14) | rustix | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [fastrand 2.5.0](https://crates.io/crates/fastrand/2.5.0) | tempfile | `Apache-2.0 OR MIT` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [getrandom 0.4.3](https://crates.io/crates/getrandom/0.4.3) | tempfile | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [libc 0.2.190](https://crates.io/crates/libc/0.2.190) | 本工程内核小库直接依赖；errno、getrandom、rustix 和 cpufeatures 的目标特有依赖 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [linux-raw-sys 0.12.1](https://crates.io/crates/linux-raw-sys/0.12.1) | 内核小库和主程序测试直接依赖；亦由 rustix 引入 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `LICENSE-MIT`、`LICENSE-APACHE`、`LICENSE-Apache-2.0_WITH_LLVM-exception`、`COPYRIGHT` |
| [once_cell 1.21.4](https://crates.io/crates/once_cell/1.21.4) | tempfile | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [r-efi 6.0.0](https://crates.io/crates/r-efi/6.0.0) | getrandom | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | `AUTHORS`（三种许可说明及版权署名），也见 `README.md` 的 License 段 |
| [rustix 1.1.5](https://crates.io/crates/rustix/1.1.5) | 主程序和内核小库直接依赖；测试依赖 tempfile 也用到 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `LICENSE-MIT`、`LICENSE-APACHE`、`LICENSE-Apache-2.0_WITH_LLVM-exception`、`COPYRIGHT` |
| [tempfile 3.27.0](https://crates.io/crates/tempfile/3.27.0) | 本工具的开发依赖 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [windows-link 0.2.1](https://crates.io/crates/windows-link/0.2.1) | windows-sys | `MIT OR Apache-2.0` | `license-mit`、`license-apache-2.0` |
| [windows-sys 0.61.2](https://crates.io/crates/windows-sys/0.61.2) | errno、rustix、tempfile | `MIT OR Apache-2.0` | `license-mit`、`license-apache-2.0` |
| [sha2 0.10.9](https://crates.io/crates/sha2/0.10.9) | 本工具直接依赖 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [cpufeatures 0.2.17](https://crates.io/crates/cpufeatures/0.2.17) | sha2 的目标特有依赖 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [digest 0.10.7](https://crates.io/crates/digest/0.10.7) | sha2 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [block-buffer 0.10.4](https://crates.io/crates/block-buffer/0.10.4) | digest 的 `core-api` | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [crypto-common 0.1.6](https://crates.io/crates/crypto-common/0.1.6) | digest | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [generic-array 0.14.7](https://crates.io/crates/generic-array/0.14.7) | block-buffer、crypto-common | `MIT` | `LICENSE` |
| [typenum 1.17.0](https://crates.io/crates/typenum/1.17.0) | crypto-common、generic-array | `MIT OR Apache-2.0` | `LICENSE`、`LICENSE-MIT`、`LICENSE-APACHE` |
| [version_check 0.9.4](https://crates.io/crates/version_check/0.9.4) | generic-array 的构建依赖 | `MIT/Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |

**还没有核实哪几项进入未来的二进制。** 锁中出现不等于实际链接：开发依赖、目标特有依赖、运行时路径分别记录。`rustix` 与 `linux-raw-sys` 的 `COPYRIGHT` 还指向个别源文件的说明；`r-efi` 的 `AUTHORS` 不是三份独立许可全文。制品实际包含哪些代码、适用的许可选项、应原样随附的版权/例外/通知，以及工具链或系统带入的材料，需在有真实制品时逐项复核。项目本身的 [Apache-2.0 许可](LICENSE)不能替代第三方材料；本表不是打包后的 notices，也不是签名。见[交付核对表](RELEASE.md)。

## SQLite 的对应源码与发行前核验

新增 10 个准确注册表归档均按 rusqlite 上游锁/发行摘录逐个与完整 SHA-256、清单和包内许可材料静态对读；只取实际选择的绑定/构建依赖，不取上游的测试、缓存、WAL、扩展加载或 SQLCipher 功能。绑定源包内的 sqlite3.c 和 sqlite3.h 是 [SQLite 3.54.1](https://sqlite.org/releaselog/3_54_1.html) 的发布材料。sqlite3.c 的 SHA3-256 为 `e2b869d3003b33c942b7dd7063604fbc53553e1b66609570df39b9b7b558ee81`，与官方 [amalgamation](https://sqlite.org/amalgamation.html) 原文件相同。MIT 针对 Rust 绑定；引擎本体的[公有领域声明](https://sqlite.org/copyright.html)及原源内声明另行保留。打包保留整个绑定归档，不将一份 MIT 文件当作引擎许可。

正式构建必须在 helper 目录使用本工程的 [.cargo/config.toml](.cargo/config.toml)，没有动态系统 SQLite 回退；实际链接、链接符号、OMIT_WAL / OMIT_LOAD_EXTENSION、连接选项、精确 ABI、归档与许可随附均待发行验收。本阶段没有运行解析器/引擎、编译 Cargo 或创建数据库。
