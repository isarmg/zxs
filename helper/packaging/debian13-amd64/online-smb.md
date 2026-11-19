# 在隔离发行机上构建两份专用 SMB 程序

**输出是两份独立的未签 ELF 材料，不是安装包。** [配方](online_smb_build.py)原生重建专用前台 smbd 和非 root 的 `zxs-smb-storage`，并为每个角色单独收集真实解释器、重建的 Samba 库和经认证的 Debian 外部 ELF。它从不执行生成的两个程序、不运行测试、不安装包或启动服务，不把这份源码变成站点启动许可。与 [helper 建域用的离线 Python 材料](README.md)无关。此仓的源码审查阶段没有执行这项配方。

### 输入与职责

1. [固定锁](online-sources.json)选择 [ZXS 已发布提交 e0ef4186](https://github.com/isarmg/zxs/commit/e0ef41866004f07c41e6472909c49f7601f1536a) 的单一项目树 `de1966fd…`。发行机必须已有只读、完整的原 Git 对象库；不读工作树、不引用相邻 checkout、不 fetch、不放宽到 main 的新版本。按原路径、mode、长度、SHA-256 核全部 255 个文件。ZXS 是产品补丁的唯一作者；helper 不再维护另一套业务补丁。
2. Debian 13 amd64 的两套只读本地镜像：稳定版根下含 `dists/trixie/`、安全仓根下含 `dists/trixie-security/`，各带相应 `pool/` 原件。两份的 main Packages/Sources 及稳定版 main/Contents-amd64.gz 都需在场。核 [Debian 签名发行索引](https://www.debian.org/doc/manuals/securing-debian-manual/deb-pack-sign.en.html)、其中的 Sources 校验和及对应的完整 `samba 2:4.22.11+dfsg-0+deb13u1` 记录，保留原签名 .dsc、orig.tar.xz 和 debian.tar.xz；[dpkg-source](https://manpages.debian.org/trixie/dpkg-dev/dpkg-source.1.en.html) 还必须能以独立受信的 Debian 开发者签名密钥环（仅有 archive keyring 不够）核 .dsc 签名与强校验后才解包。保留原件便于重核，不从未修改的系统二进制、未打 Debian quilt 的上游 tar 或脱离该套源码的安全补丁拼装。固定历史快照不是“目前无漏洞”的证明。发布前另核该版及依赖的最新安全状态；需要更新时整套重钉重新复核。
3. 干净、独占、禁网的 Debian 13 amd64 **一次性构建机**，普通非 root 账号，对业务设备/密钥/现场服务无访问。独立验证整个不可变构建镜像、Debian 软件/构建依赖与 keyring、完整 Rust 1.100.0 工具链与标准库；不能光靠版本字符串。Rust/Cargo 固定在 `/opt/zxs-rust-1.100.0/bin`，不用 rustup 在线补装。构建者独占且有权新建 `/build/zxs-smb-v1`，它须不存在，不能是其他 Git 仓库的子目录；父目录及工具配置不得含未批准的 Cargo/Git/编译器设置。配方使用固定工位路径、UTC、C locale、SOURCE_DATE_EPOCH、干净 HOME、单作业及离线锁定 Cargo，帮助复现；未做双构建比对，不能声称位对位可复现。
4. 在独立受信参考构建镜像可先用 `online_smb_build.py --describe-environment` 取得 `BUILD-ENV-2`：实际安装的包/架构/版本、Rust/Cargo 的真实版本输出及所列构建驱动的实际文件摘要。审核后将其 SHA-256 带外给这次构建者；不要在待验机器现算现批。此记录不是启动器、也不能代替整个镜像及实际工具链/依赖的真实性证明。旧 BUILD-ENV-1 不能代替本版带 dpkg-deb/dpkg-query 的记录。另备已独立核过的工具链/链接材料的对应源和版权归档及 SHA-256；脚本仅限量保全原字节，不替发行方鉴定法律完备性。

### 仅在另行批准实际构建时

```sh
/usr/bin/python3 -I -S -B /release/helper/packaging/debian13-amd64/online_smb_build.py \
  --project-git /inputs/zxs.git --debian-mirror /inputs/debian \
  --debian-security-mirror /inputs/debian-security \
  --environment-sha256 "$REVIEWED_ENV_SHA256" \
  --toolchain-materials /inputs/toolchain-source-and-notices.tar.xz \
  --materials-sha256 "$REVIEWED_MATERIALS_SHA256"
```

先由 Debian 解出**已应用发行补丁**的源树。对产品的 `inputs.sha256` 每项核原始字节（包括只验证、不修改的头）；`new/` 只建尚不存在的准确文件，已有同名件直接失败。只使用那一份 `0001-managed-standalone.patch`，禁止模糊匹配，复核实际改动范围。Samba 调原 Debian `debian/rules build-arch`，取实际安装阶段重新链接到私有工作区 `debian/tmp` 的 smbd 和库；不取开发目录的可执行物，不将该 staging 安装到宿主。此构建会运行 Debian 上游发行工具、必要的生成器和编译器，不运行 smbd 或存储服务。存储用同一产品源的 Cargo.lock、build.rs、随源 SQLite 和原控制通道 C 编译，仅 `cargo build --frozen --release --target x86_64-unknown-linux-gnu --bin zxs-smb-storage`。

### 如何读结果

- `sources/` 保留完整 ZXS 精确原件及逐文件清单、Debian 对应源/补丁/打包说明和签名索引、实际配方、补后源件摘要、环境记录与工具链材料。原 Samba/Debian 版权、GPL 许可、ZXS 的 Apache 声明、SQLite 来历另列；这些不穷尽一份最终可再分发软件包的材料义务。见 [Debian 对应源包](https://www.debian.org/doc/debian-policy/ch-source.html)、[GNU 对应源码](https://www.gnu.org/licenses/gpl-3.0.html#section6)。
- `objects/usr/lib/zxs-smb-runtime/debian13-amd64/{sbin/smbd,bin/zxs-smb-storage}` 是本次构建的两个角色。收集器用 objcopy 去调试信息，用 readelf 核 amd64 ELF、解释器、每件的 NEEDED/搜索路径和库 SONAME，仅从本次 Debian 编译 staging 的标准 multiarch/专用 Samba 目录带走实际可达的**重建库**；绝不在构建者宿主全盘找同名库，不用 ldd。对象目录不是可运行的根视图。
- `UNSIGNED-OBJECTS.json` 仍是**中间**清单；其中“未供应的 SONAME”只指从重建物走出的第一组边，不是最终结果。下一步在同一进程、同一私有构建目录处理实际对象，不接受从别处提交一份无来源的 JSON。
- `elf-roles/smbd` 和 `elf-roles/storage` 各自只含一个角色的可执行文件及其 ELF 链接所需的库。按 [动态加载器实际的 NEEDED 与搜索路径](https://man7.org/linux/man-pages/man8/ld.so.8.html)在该角色中逐件读实际字节；每份解释器只接受 [libc6 的原 amd64 loader](https://packages.debian.org/trixie/amd64/libc6/filelist)，固定 `/lib64/ld-linux-x86-64.so.2` 与该视图的 multiarch 名是**同一 inode**。无 `ldd`、无运行 ELF、无主机 /lib、缓存、LD_LIBRARY_PATH、任意 basename 查找或 stock Samba 退路。普通发行库只在规范 multiarch 目录内按原 SONAME 查；拒未知搜索规则、不同来源同名库或缺传递依赖。同一 SONAME 不保证所需 GNU 符号版本或重定位可用；本批未核 .gnu.version* ABI，仍须单独验证实际加载与启动。
- 外部 DSO 的包名先取自稳定版签名 Contents，并须在独立核准环境中唯一；再按两套签名 Packages 中的准确架构、版本、实际 .deb 字节核对及在原包内解开精确文件。Contents **不是**文件本身的证据。若安全更新新增了旧 Contents 不能归属的文件、环境包与锁定快照不符，或两仓同版本不同字节，均停止，不借宿主补。本锁安全仓 InRelease 有效至 **2026-11-26 16:43:51 UTC**。编译开始前及最终记清单前均查有效期；过期需刷新完整发行锁并独立批准，不因重现旧版本而绕过。
- `sources/external/` 留所用的签名索引、原 .deb、按 Source 的**源版本**交叉查回的 .dsc、完整原始 tar/发行补丁及打包材料。它还带实际供应包的 copyright 与 base-files 的通用许可正文；仅同源同版本的 Debian 包可补版权目录别名（例如 libgcc 的 gcc-14-base）。稳定仓只接其原 `pool/main/`，安全仓只接 `pool/updates/main/`；同一源版本两仓都记录时，按全套归档**文件名、大小和 SHA-256** 比对，不把不同的仓路径误报成不同源码。若相同优先保留稳定仓原路径；相异拒绝。这一步不安装包、不执行维护脚本。索引及文件认证链参见 [apt-secure](https://manpages.debian.org/trixie/apt/apt-secure.8.en.html)。不能由技术清单代替最终再发行法律复核。
- 最后 `UNSIGNED-ELF-ROLES.json` 记每个角色的路径、来源、模式、长度、SHA-256、解释器同物关系、各条 NEEDED 去向，所用 Debian 二进制/源包版本和实际原档摘要，且绑定此次输入锁/构建环境和中间清单。只有两边都闭合才写该文件并移去 `INCOMPLETE`。它仍不是签章、安装权限或 `/etc` 的站点准许。
- **下一道门：** 角色材料还不是可交主管的只读运行根。必须另核其中动态请求的 NSS/字符集/配置、Samba 模块或认证原语，以及实际所需受保护的运行目录、数据投射、内存和停机条件；不得替换成宿主目录。最终发行还需在隔离机双构建、审版本安全及版权、独立验签、全依赖冷启动和业务实测；此前不签两 ELF 为可上线，不尝试登录或对外共享。
