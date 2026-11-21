# 在隔离发行机上构建两份专用 SMB 程序

**输出是两个独立角色的未签材料，不是安装包。** [配方](online_smb_build.py)原生重建专用前台 smbd 和非 root 的 `zxs-smb-storage`，并为每个角色单独收集真实解释器、重建的 Samba 库和经认证的 Debian 外部 ELF。仅 smbd 另有初始化实际用到的本地身份记录、字符集模块和精简转换表。它从不执行生成的两个程序、不运行测试、不安装包或启动服务，不把这份源码变成站点启动许可。与 [helper 建域用的离线 Python 材料](README.md)无关。此仓的源码审查阶段没有执行这项配方。

### 输入与职责

1. [固定锁](online-sources.json)选择单一原生 SMB 3.1.1 的 v16 ZXS 树 `9e36fb9d80d2e7eb4cc24f17d3fe644c8a5806e6`。同一正式 v16 的 ZXS 源码还精确呈现实际只读挂载与内核文件系统标识，以及仅 ext4 可得的短窗写入计数增量与已记录的文件系统错误。后两项不是测速或硬件健康；该状态更新是只读 Web 源，不代表此配方会发布 Web 制品或验收。用 `--project-commit` 提供包含这棵精确树的已审核共同源码提交，不用当前主线代替。配对源码新增普通子目录在**同一真实父目录**下无覆盖改名：使用当前 RW 共享上具备 DELETE 权限的实际原句柄，保留 DirectoryId、实体和子项历史；预检遇忙、有名称冲突或不能证明原物时不开始；中断后辨认同一原单，不凭名字推断完成。保留 v15 普通 FILE 同盘改名及同一 ZXS 内跨真实文件系统搬移能力；不扩为目录跨父迁移、目录成批回收、加盘或退登记。两端仍只接受逐字相同的 v16 SQLite，只建新池，不迁移或接管旧库，不切换到实验 Rust SMB。Native v15 的配方与已发布源码留在 [a665f736](https://github.com/isarmg/zxs/commit/a665f736c65b9cfe4d3179b61882067561df1f82)，不是此版输入。发行机按原路径、模式、长度和 SHA-256 核全部 1165 个文件，按原 Debian 索引及完整摘要核 61 项 Samba 源件；必须已有只读且完整的原 Git 对象库，不读工作树、不引用相邻 checkout、不 fetch。ZXS 是产品补丁的唯一作者；helper 不另维护业务补丁。已核源码，未执行构建、挂载或客户端验证。
2. Debian 13 amd64 的两套只读本地镜像：稳定版根下含 `dists/trixie/`、安全仓根下含 `dists/trixie-security/`，各带相应 `pool/` 原件。两份的 main Packages/Sources 及稳定版 main/Contents-amd64.gz 都需在场。核 [Debian 签名发行索引](https://www.debian.org/doc/manuals/securing-debian-manual/deb-pack-sign.en.html)、其中的 Sources 校验和及对应的完整 `samba 2:4.22.11+dfsg-0+deb13u1` 记录，保留原签名 .dsc、orig.tar.xz 和 debian.tar.xz；[dpkg-source](https://manpages.debian.org/trixie/dpkg-dev/dpkg-source.1.en.html) 还必须能以独立受信的 Debian 开发者签名密钥环（仅有 archive keyring 不够）核 .dsc 签名与强校验后才解包。保留原件便于重核，不从未修改的系统二进制、未打 Debian quilt 的上游 tar 或脱离该套源码的安全补丁拼装。固定历史快照不是“目前无漏洞”的证明。发布前另核该版及依赖的最新安全状态；需要更新时整套重钉重新复核。
3. 干净、独占、禁网的 Debian 13 amd64 **一次性构建机**，普通非 root 账号，对业务设备/密钥/现场服务无访问。独立验证整个不可变构建镜像、Debian 软件/构建依赖与 keyring、完整 Rust 1.100.0 工具链与标准库；不能光靠版本字符串。Rust/Cargo 固定在 `/opt/zxs-rust-1.100.0/bin`，不用 rustup 在线补装。构建者独占且有权新建 `/build/zxs-smb-v1`，它须不存在，不能是其他 Git 仓库的子目录；父目录及工具配置不得含未批准的 Cargo/Git/编译器设置。配方使用固定工位路径、UTC、C locale、SOURCE_DATE_EPOCH、干净 HOME、单作业及离线锁定 Cargo，帮助复现；未做双构建比对，不能声称位对位可复现。
4. 在独立受信参考构建镜像可先用 `online_smb_build.py --describe-environment` 取得 `BUILD-ENV-2`：实际安装的包/架构/版本、Rust/Cargo 的真实版本输出及所列构建驱动的实际文件摘要。审核后将其 SHA-256 带外给这次构建者；不要在待验机器现算现批。此记录不是启动器、也不能代替整个镜像及实际工具链/依赖的真实性证明。旧 BUILD-ENV-1 不能代替本版带 dpkg-deb/dpkg-query 的记录。另备已独立核过的工具链/链接材料的对应源和版权归档及 SHA-256；脚本仅限量保全原字节，不替发行方鉴定法律完备性。

5. 站点独立批准的文件 `ZXS-SMB-UNIX-2` 和带外 SHA-256 是本次构建的必要输入；不从构建机/宿主 NSS 推断。维护者以真实原申请取普通存储（也是 ZXS 原生 HTTP 的运行身份）UID/GID，另分配独立的 SMB 执行 UID/GID；这些用户号和组号分别不能重叠，不能占用 0、65534、无效号或现场任何其他主体已授权的号码。批准声明只含数字，不含密码或共享权。固定格式如下（只是合成数字，不应照抄；十进制、无前置零、以 LF 结束）：

```text
ZXS-SMB-UNIX-2
storage 2001 2002
smb-execution 4001 4002
```

没有配置或摘要、名字/序次/值不合、与部署的储存/代理身份不符都不能采用这份 smbd 角色。若站点换一项数字，要重新以独立已批准原文件组装、核验角色和原发行记录；旧摘要不能沿用。这不创建宿主 Unix 账户。

### 仅在另行批准实际构建时

```sh
/usr/bin/python3 -I -S -B /release/helper/packaging/debian13-amd64/online_smb_build.py \
  --project-git /inputs/zxs.git --project-commit "$VERIFIED_COORDINATED_COMMIT" --debian-mirror /inputs/debian \
  --debian-security-mirror /inputs/debian-security \
  --environment-sha256 "$REVIEWED_ENV_SHA256" \
  --toolchain-materials /inputs/toolchain-source-and-notices.tar.xz \
  --materials-sha256 "$REVIEWED_MATERIALS_SHA256" \
  --site-identities /inputs/site-smb-identities --site-identities-sha256 "$APPROVED_SITE_IDENTITIES_SHA256"
```

先由 Debian 解出**已应用发行补丁**的源树。对产品的 `inputs.sha256` 每项核原始字节（包括只验证、不修改的头）；`new/` 只建尚不存在的准确文件，已有同名件直接失败。只使用那一份 `0001-managed-standalone.patch`，禁止模糊匹配，复核实际改动范围。Samba 调原 Debian `debian/rules build-arch`，取实际安装阶段重新链接到私有工作区 `debian/tmp` 的 smbd 和库；不取开发目录的可执行物，不将该 staging 安装到宿主。此构建会运行 Debian 上游发行工具、必要的生成器和编译器，不运行 smbd 或存储服务。存储用同一产品源的 Cargo.lock、build.rs、随源 SQLite 和原控制通道 C 编译，仅 `cargo build --frozen --release --target x86_64-unknown-linux-gnu --bin zxs-smb-storage`。

### 如何读结果

- `sources/` 保留完整 ZXS 精确原件及逐文件清单、Debian 对应源/补丁/打包说明和签名索引、实际配方、补后源件摘要、环境记录与工具链材料。原 Samba/Debian 版权、GPL 许可、ZXS 的 Apache 声明、SQLite 来历另列；这些不穷尽一份最终可再分发软件包的材料义务。见 [Debian 对应源包](https://www.debian.org/doc/debian-policy/ch-source.html)、[GNU 对应源码](https://www.gnu.org/licenses/gpl-3.0.html#section6)。
- `objects/usr/lib/zxs-smb-runtime/debian13-amd64/{sbin/smbd,bin/zxs-smb-storage}` 是本次构建的两个角色。收集器用 objcopy 去调试信息，用 readelf 核 amd64 ELF、解释器、每件的 NEEDED/搜索路径和库 SONAME，仅从本次 Debian 编译 staging 的标准 multiarch/专用 Samba 目录带走实际可达的**重建库**；绝不在构建者宿主全盘找同名库，不用 ldd。对象目录不是可运行的根视图。
- `UNSIGNED-OBJECTS.json` 仍是**中间**清单；其中“未供应的 SONAME”只指从重建物走出的第一组边，不是最终结果。下一步在同一进程、同一私有构建目录处理实际对象，不接受从别处提交一份无来源的 JSON。
- `elf-roles/smbd` 和 `elf-roles/storage` 各自只含本角色的一个可执行文件及其所需链接库；下述额外启动材料只进 smbd。按 [动态加载器实际的 NEEDED 与搜索路径](https://man7.org/linux/man-pages/man8/ld.so.8.html)在该角色中逐件读实际字节；每份解释器只接受 [libc6 的原 amd64 loader](https://packages.debian.org/trixie/amd64/libc6/filelist)，固定 `/lib64/ld-linux-x86-64.so.2` 与该视图的 multiarch 名是**同一 inode**。无 `ldd`、无运行 ELF、无主机 /lib、缓存、LD_LIBRARY_PATH、任意 basename 查找或 stock Samba 退路。普通发行库只在规范 multiarch 目录内按原 SONAME 查；拒未知搜索规则、不同来源同名库或缺传递依赖。同一 SONAME 不保证所需 GNU 符号版本或重定位可用；本批未核 .gnu.version* ABI，仍须单独验证实际加载与启动。
- 外部 DSO 的包名先取自稳定版签名 Contents，并须在独立核准环境中唯一；再按两套签名 Packages 中的准确架构、版本、实际 .deb 字节核对及在原包内解开精确文件。Contents **不是**文件本身的证据。若安全更新新增了旧 Contents 不能归属的文件、环境包与锁定快照不符，或两仓同版本不同字节，均停止，不借宿主补。本锁安全仓 InRelease 有效至 **2026-11-26 16:43:51 UTC**。编译开始前及最终记清单前均查有效期；过期需刷新完整发行锁并独立批准，不因重现旧版本而绕过。
- `sources/external/` 留所用的签名索引、原 .deb、按 Source 的**源版本**交叉查回的 .dsc、完整原始 tar/发行补丁及打包材料。它还带实际供应包的 copyright 与 base-files 的通用许可正文；仅同源同版本的 Debian 包可补版权目录别名（例如 libgcc 的 gcc-14-base）。稳定仓只接其原 `pool/main/`，安全仓只接 `pool/updates/main/`；同一源版本两仓都记录时，按全套归档**文件名、大小和 SHA-256** 比对，不把不同的仓路径误报成不同源码。若相同优先保留稳定仓原路径；相异拒绝。这一步不安装包、不执行维护脚本。索引及文件认证链参见 [apt-secure](https://manpages.debian.org/trixie/apt/apt-secure.8.en.html)。不能由技术清单代替最终再发行法律复核。
- 最后 `UNSIGNED-ELF-ROLES.json` 记每个角色的路径、来源、模式、长度、SHA-256、解释器同物关系、各条 NEEDED 去向，所用 Debian 二进制/源包版本和实际原档摘要，且绑定此次输入锁/构建环境、中间清单及站点原身份文件的批准摘要。构建器在各自新树上枚举实际名字、完整重读每件文件，对照各自清单核类型、权限、链接数、长度和字节；解释器两名须同物，多一件也拒。它不扫描宿主根、不检查以后另作的部署投射。只有两边都闭合才写该文件并移去 `INCOMPLETE`。它仍不是签章、安装权限或 `/etc` 的站点准许。
- **下一道门：** 角色材料还不是可交主管的只读运行根。必须按真实站点核批准的低权限执行号、最终实际加载/认证能力、受保护的运行目录、数据投射、内存和停机条件；不得替换成宿主目录。最终发行还需在隔离机双构建、审版本安全及版权、独立验签、全依赖冷启动和业务实测；此前不签两 ELF 为可上线，不尝试登录或对外共享。


### 专用 smbd 的初始化材料

- **本地身份来源。** [glibc 2.41 的 files 后端](https://github.com/bminor/glibc/blob/glibc-2.41/nss/nss_module.c)编入 libc；这不等于无配置时会只查文件。[该版 NSS 的默认选项](https://github.com/bminor/glibc/blob/glibc-2.41/nss/nss_database.c)可使用 DNS/compat。配方生成角色内部的 `/etc/nsswitch.conf`，逐项把[全部 17 个数据库](https://github.com/bminor/glibc/blob/glibc-2.41/nss/databases.def)指定为 files，含 passwd、group、initgroups、hosts 和 networks；不单取兼容的 libnss_files.so，不复制宿主配置、DNS/LDAP/NSS 模块、PAM 栈、密码文件或 nscd 套接字。部署时也不能投射宿主 /run、/var/run、nscd 或额外的 /etc。此策略约束 NSS 选择，不是网络隔离的替代。
- **本机编号不等于网络账户。** [原版 smbd](https://github.com/samba-team/samba/blob/samba-4.22.11/source3/smbd/server.c)在接收客户前即需建立系统和内部 guest 令牌；[其实现](https://github.com/samba-team/samba/blob/samba-4.22.11/source3/auth/auth_util.c)按本地身份查询。角色私有 passwd/group 包含初始化用的 root(0:0)、nobody(65534:65534，组 nogroup) 和独立批准的唯一 `zxs-smb-client` 数值执行身份；所有行都锁定密码，无补充组、无 shell 程序，也不复制任何宿主账户。角色还保留摘要已核的原身份证明 `/etc/zxs-smb-identities`。主管在产生两位子进程前以实际只读角色根复核这四份 NSS 文件和之前独立核的存储/代理身份；存储角色不取得该映射。这不是产品账号或登录权，也不会开放匿名登录。
- **字符集为什么带两件。** 固定配置的 Unix 字符集是 UTF-8、DOS 是 CP850。Samba 自有 UTF-8/UTF-16 处理不覆盖此代码页；[iconv.c](https://github.com/samba-team/samba/blob/samba-4.22.11/lib/util/charset/iconv.c)会向系统请求 CP850 与 UTF-16LE 的双向转换。配方只从准确的 amd64 libc6 2.41-12+deb13u4 原包取 `IBM850.so`、`UTF-16.so`，核 ELF 中实际定义的可调用 gconv 符号，递归核两者的 NEEDED 和供应者；专路径加载不假设模块一定有 SONAME。从原包[基础表](https://github.com/bminor/glibc/blob/glibc-2.41/iconvdata/gconv-modules)与[额外表](https://github.com/bminor/glibc/blob/glibc-2.41/iconvdata/gconv-modules-extra.conf)核并仅生成五条规则（CP850 别名、IBM850/UTF-16LE 分别与 INTERNAL 双向连接），在清单内留下两份完整输入表的来源和摘要。不投射剩余模块、缓存、GCONV_PATH 或构建机的数据库。Samba 大小写表已在[codepoints.c](https://github.com/samba-team/samba/blob/samba-4.22.11/lib/util/charset/codepoints.c)编入；不需要额外的 upcase.dat/lowcase.dat。
- **未验证的边界。** 没有把普通发行版 Samba 的 passdb、VFS 或认证插件作为候选；这份补丁固定域、只读原生 tdbsam 和 Samba 自己的认证回调与 `zxs_file`，不从宿主加载另一套实现。配方本身不验证凭据、不启动服务；正确来源和角色根仍不证明成功网络登录。存储角色不带上述 smbd 专用材料。签发前仍须在独立隔离的真实角色根验证不走宿主服务、NSS 按名称及编号都能查到准确的三个角色内身份（无附组）、以及非 ASCII 的 CP850 字节 82 与 UTF-16LE E9 00 双向一致；仅进程启动或 ASCII 通过无法排除上游悄悄退回 ASCII。以上验收、实际认证、测试和签章本次均未执行。


运行侧仅消耗独立验证后固定安装的 `ELF-ROLES-1` 与两棵 `elf-roles/{smbd,storage}`。该窄清单逐件绑定来源整树、实际共用数据库定义、模式、字节数和 SHA-256，不从站点自报、宿主全局库或可换的 JSON 选其他原件。独立发行者签章和托管仍是必要步骤；[正式启动](../../docs/smb-host.md) 不由配方执行。
