# 依赖与固定版本

[Cargo.toml](Cargo.toml) 在主程序中声明两个非开发的直接注册表依赖，另有一个本项目内部小库：

- `rustix = 1.1.5`（默认 `std`，显式 `fs`、`rand`）：安全的 Linux 文件描述符和文件系统观察；新池/卷 ID 只使用内核非阻塞 getrandom。本次仅启用同一锁定包的现有功能，没有增加依赖或改锁。
- `sha2 = 0.10.9`（不启用默认功能）：在内存中以 RustCrypto SHA-256 核验候选新登记完整前缀；不用于认证操作者或保护口令。

主程序的测试直接用 `linux-raw-sys 0.12.1` 核已知 Linux 文件系统类型。`tempfile = 3.27.0` 仅供测试，在专用临时父目录内独占创建私有子目录；它不是运行时组件。位置参数、字节流和输出使用标准库。支持的 Rust 工具链单独固定为 1.100.0。

本项目内部的 [helper-linux-evidence](linux-evidence/Cargo.toml) 是单独静审的 Linux LP64 小库，不依赖其他项目。它直接使用锁内的 `libc 0.2.190`、`linux-raw-sys 0.12.1` 和 `rustix 1.1.5`。主程序仍禁止 unsafe；小库仅在三项固定 Linux/POSIX 只读接口处使用 unsafe：通过持有的原描述符读取 [FS_IOC_GETFSUUID](https://github.com/torvalds/linux/blob/master/include/uapi/linux/fs.h) 和 [name_to_handle_at](https://www.man7.org/linux/man-pages/man2/name_to_handle_at.2.html)；每个卷行的原生名称字节限额使用同目录 [fpathconf(_PC_NAME_MAX)](https://man7.org/linux/man-pages/man3/fpathconf.3.html)，不猜未知值。没有任意 ioctl、路径重开或按句柄开文件的接口。libc 现在是小库的直接普通依赖，不是新增或升级了注册表包。

本工具的 13 项原有注册表记录取自 **cc 1.8.0** 官方发行包的 Cargo.lock，未改版本。新增 8 项按 **[sha2 0.10.9](https://crates.io/crates/sha2/0.10.9)** 官方发行包的 Cargo.lock 固定版本，逐个与 crates.io 原档案、登记校验值及其规范化 Cargo.toml 核对。锁内记录本工程选用的依赖，包括本工程的测试依赖及所需的构建依赖；不引 sha2 自身的测试、可选的汇编/OID，亦不引 digest 的测试数据。共用 cfg-if 和 libc 的原锁版本。这份锁不是在本工程新运行解析而得。跨平台传递记录不表示支持其他平台。

在独立目录使用标准 crates.io 源，或使用已备齐精确版本的本机可信缓存；不从父目录导入清单、源码、锁文件或构建产物。缓存不全时显式报告缺件，不替换包名或校验值。没有捆绑或安装第三方二进制。

## 已核对的 21 份发行源码包

本表对应本项目 [Cargo.lock](Cargo.lock)（文件 SHA-256：`e8dee9ae171904383c74ee74595cd4fd4770c4795a213c36a3c6e8ad100d1045`）。逐个从 crates.io 取了锁定版本的 `.crate`，先核整个归档 SHA-256 与锁里该包的 `checksum` 相同，再静态读取归档中的 `Cargo.toml`、许可及署名材料。没有安装、执行或重新解析这些包。表内链接指向各版本的注册表页面；[Cargo 注册表规范](https://doc.rust-lang.org/cargo/reference/registry-index.html#download-endpoint)给出原始包的下载方式。

| 精确发行包 | 谁在锁里引用它 | 包声明的许可表达式 | 包内已核材料 |
| --- | --- | --- | --- |
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
