# 依赖与固定版本

供维护者核对源码和交付材料。先备齐 `Cargo.lock` 固定的版本及对应许可。
这里记录独立 Helper 的依赖和来源；锁定不代表已核验发行制品。

[Cargo.toml](Cargo.toml) 在主程序中声明六个非开发的直接注册表依赖，另有一个本项目内部小库：

- `argon2 = 0.5.3`（只开 `zeroize`）、`zeroize = 1.9.1`（只开 `alloc`）：前台首设及密码重置使用固定格式的慢验证值，使用 RustCrypto 上游算法及有界、显式擦除的工作区；不导入 PHC 文本、外部参数或口令生成器。前台两次输入的字节比较另直接使用锁内的 `subtle = 2.6.1`（不启用默认功能）。
- `rustix = 1.1.5`（默认 `std`，显式 `event`、`fs`、`mm`、`net`、`pipe`、`process`、`rand`、`termios`、`thread`）：提供 Linux 系统调用封装，包括文件描述符和文件系统观察、内核随机数及凭据进程的内存锁定。测试另开 `pty` 和 `stdio`。
- `sha2 = 0.10.9`（不启用默认功能）：使用 RustCrypto SHA-256 为导入原件、计划和离线请求计算摘要，并核对可选内嵌制品；不用于认证操作者或保护口令。
- `linux-raw-sys = 0.12.1`（关闭默认功能，显式 `general`、`std`）：提供挂载准入和文件原件核对使用的 Linux 文件系统类型常量。

`tempfile = 3.27.0` 仅供测试，在专用临时父目录内独占创建私有子目录。Rust 工具链固定为 1.100.0。

本项目内的 [helper-linux-evidence](linux-evidence/Cargo.toml) 直接使用锁内的 `libc 0.2.190`、`linux-raw-sys 0.12.1` 和 `rustix 1.1.5`。
它不依赖其他项目。文件系统类型准入仅查内核值白名单，实际挂载和原件仍由主程序核对。

主程序禁止 `unsafe`。小库仅在固定 Linux 进程和信号边界使用 `unsafe`：核对可等待的子进程，通过
[`close_range`](https://man7.org/linux/man-pages/man2/close_range.2.html) 设置非标准描述符的关闭标记，仅保留各固定协议允许的描述符，
并对已核的 ELF 描述符使用 `execveat`。离线名称读取进程建立独立会话。

专用单线程前台输入由小库检查中断信号并恢复原线程掩码；该封装不读取凭据。
这些接口不提供按路径选择程序、通用设备控制或按文件句柄重开。

本项目的锁文件包括主程序、内部小库、构建和测试所需的注册表包。支持目标为 Linux x86_64；表中另列锁内的目标特有传递依赖。

在独立目录使用标准 crates.io 源，或使用已备齐精确版本的本机可信缓存；不从父目录导入清单、源码、锁文件或构建产物。缓存不全时显式报告缺件，不替换包名或校验值。本次未捆绑或安装第三方二进制。旧卷镜像工具和网页的来源见[历史索引](docs/archive/retired-volume-tools.md)，它们不是现行发行的一部分。

## [仅静态] 已核对的 26 份发行源码包

Cargo.lock SHA-256: `9dea3e14264ad491146cc648e01184a152209aa262ab3970ef441b0bfa36475d`

本表使用 [Cargo.lock](Cargo.lock) 中的版本和 `checksum` 核对整个 `.crate` 归档，许可及署名以归档材料为准。[Cargo 注册表规范](https://doc.rust-lang.org/cargo/reference/registry-index.html#download-endpoint)给出原始包下载方式。

| 精确发行包 | 谁在锁里引用它 | 包声明的许可表达式 | 包内已核材料 |
| --- | --- | --- | --- |
| [argon2 0.5.3](https://crates.io/crates/argon2/0.5.3) | 主程序；固定 Argon2id v19 | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [base64ct 1.8.3](https://crates.io/crates/base64ct/1.8.3) | argon2；本项目不让操作者提供 PHC 参数串 | `Apache-2.0 OR MIT` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [blake2 0.10.6](https://crates.io/crates/blake2/0.10.6) | argon2 | `MIT OR Apache-2.0` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [subtle 2.6.1](https://crates.io/crates/subtle/2.6.1) | 主程序直接使用，亦由 blake2 所用 digest 的 mac 特性引入 | `BSD-3-Clause` | `LICENSE` |
| [zeroize 1.9.1](https://crates.io/crates/zeroize/1.9.1) | 主程序及 argon2 的指定特性 | `Apache-2.0 OR MIT` | `LICENSE-APACHE`、`LICENSE-MIT` |
| [bitflags 2.13.2](https://crates.io/crates/bitflags/2.13.2) | rustix | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [cfg-if 1.0.5](https://crates.io/crates/cfg-if/1.0.5) | getrandom、sha2 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [errno 0.3.14](https://crates.io/crates/errno/0.3.14) | rustix | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [fastrand 2.5.0](https://crates.io/crates/fastrand/2.5.0) | tempfile | `Apache-2.0 OR MIT` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [getrandom 0.4.3](https://crates.io/crates/getrandom/0.4.3) | tempfile | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [libc 0.2.190](https://crates.io/crates/libc/0.2.190) | 本工程内核小库直接依赖；errno、getrandom、rustix 和 cpufeatures 的目标特有依赖 | `MIT OR Apache-2.0` | `LICENSE-MIT`、`LICENSE-APACHE` |
| [linux-raw-sys 0.12.1](https://crates.io/crates/linux-raw-sys/0.12.1) | 主程序和内核小库直接依赖；亦由 rustix 引入 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `LICENSE-MIT`、`LICENSE-APACHE`、`LICENSE-Apache-2.0_WITH_LLVM-exception`、`COPYRIGHT` |
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

## 退役材料的来源记录

不再构建 SQLite/SMB 历史夹具或分发旧凭据提供者。过去版本的[锁定源码及许可核对](https://github.com/isarmg/zxs/blob/b2e3af5aba9c0901bfaa83c4bebf16d75a25e79e/helper/DEPENDENCIES.md)和[原 Debian 包摘要](https://github.com/isarmg/zxs/blob/b2e3af5aba9c0901bfaa83c4bebf16d75a25e79e/helper/packaging/debian13-amd64/lock.json)仍可按该提交只读查证。这些材料不属于当前锁或发行配方。
