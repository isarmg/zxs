# 在隔离发行机上构建两份专用 SMB 程序

**输出是未签的构建中间件，不是安装包。** [配方](online_smb_build.py)原生重建专用前台 smbd 和非 root 的 `zxs-smb-storage`，并按它们的链接关系收集这次重建的 Samba 库。它从不执行生成的两个程序、不运行测试、不安装包或启动服务，不把这份源码变成站点启动许可。与 [helper 建域用的离线 Python 材料](README.md)无关。此仓的源码审查阶段没有执行这项配方。

### 输入与职责

1. [固定锁](online-sources.json)选择 [ZXS 已发布提交 dec7aa55](https://github.com/isarmg/zxs/commit/dec7aa55fa9fa5789d75a89feccb3024d41880bb) 的单一项目树 `0da4ba10…`。发行机必须已有只读、完整的原 Git 对象库；不读工作树、不引用相邻 checkout、不 fetch、不放宽到 main 的新版本。按原路径、mode、长度、SHA-256 核全部 254 个文件。ZXS 是产品补丁的唯一作者；helper 不再维护另一套业务补丁。
2. Debian 13 amd64 的本地已认证镜像输入（根下含 `dists/trixie/` 和 `pool/`）。核 [Debian 签名发行索引](https://www.debian.org/doc/manuals/securing-debian-manual/deb-pack-sign.en.html)、其中的 Sources 校验和及对应的完整 `samba 2:4.22.11+dfsg-0+deb13u1` 记录，保留原签名 .dsc、orig.tar.xz 和 debian.tar.xz；[dpkg-source](https://manpages.debian.org/trixie/dpkg-dev/dpkg-source.1.en.html) 还必须能以独立受信的 Debian 密钥核 .dsc 签名与强校验后才解包。保留原件便于重核，不从未修改的系统二进制、未打 Debian quilt 的上游 tar 或脱离该套源码的安全补丁拼装。固定历史快照不是“目前无漏洞”的证明。发布前另核该版及依赖的最新安全状态；需要更新时整套重钉重新复核。
3. 干净、独占、禁网的 Debian 13 amd64 **一次性构建机**，普通非 root 账号，对业务设备/密钥/现场服务无访问。独立验证整个不可变构建镜像、Debian 软件/构建依赖与 keyring、完整 Rust 1.100.0 工具链与标准库；不能光靠版本字符串。Rust/Cargo 固定在 `/opt/zxs-rust-1.100.0/bin`，不用 rustup 在线补装。构建者独占且有权新建 `/build/zxs-smb-v1`，它须不存在，不能是其他 Git 仓库的子目录；父目录及工具配置不得含未批准的 Cargo/Git/编译器设置。配方使用固定工位路径、UTC、C locale、SOURCE_DATE_EPOCH、干净 HOME、单作业及离线锁定 Cargo，帮助复现；未做双构建比对，不能声称位对位可复现。
4. 在独立受信参考构建镜像可先用 `online_smb_build.py --describe-environment` 取得 `BUILD-ENV-1`：实际安装的包/架构/版本、Rust/Cargo 的真实版本输出及所列构建驱动的实际文件摘要。审核后将其 SHA-256 带外给这次构建者；不要在待验机器现算现批。此记录不是启动器、也不能代替整个镜像及实际工具链/依赖的真实性证明。另备已独立核过的工具链/链接材料的对应源和版权归档及 SHA-256；脚本仅限量保全原字节，不替发行方鉴定法律完备性。

### 仅在另行批准实际构建时

```sh
/usr/bin/python3 -I -S -B /release/helper/packaging/debian13-amd64/online_smb_build.py \
  --project-git /inputs/zxs.git --debian-mirror /inputs/debian \
  --environment-sha256 "$REVIEWED_ENV_SHA256" \
  --toolchain-materials /inputs/toolchain-source-and-notices.tar.xz \
  --materials-sha256 "$REVIEWED_MATERIALS_SHA256"
```

先由 Debian 解出**已应用发行补丁**的源树。对产品的 `inputs.sha256` 每项核原始字节（包括只验证、不修改的头）；`new/` 只建尚不存在的准确文件，已有同名件直接失败。只使用那一份 `0001-managed-standalone.patch`，禁止模糊匹配，复核实际改动范围。Samba 调原 Debian `debian/rules build-arch`，取实际安装阶段重新链接到私有工作区 `debian/tmp` 的 smbd 和库；不取开发目录的可执行物，不将该 staging 安装到宿主。此构建会运行 Debian 上游发行工具、必要的生成器和编译器，不运行 smbd 或存储服务。存储用同一产品源的 Cargo.lock、build.rs、随源 SQLite 和原控制通道 C 编译，仅 `cargo build --frozen --release --target x86_64-unknown-linux-gnu --bin zxs-smb-storage`。

### 如何读结果

- `sources/` 保留完整 ZXS 精确原件及逐文件清单、Debian 对应源/补丁/打包说明和签名索引、实际配方、补后源件摘要、环境记录与工具链材料。原 Samba/Debian 版权、GPL 许可、ZXS 的 Apache 声明、SQLite 来历另列；这些不穷尽一份最终可再分发软件包的材料义务。见 [Debian 对应源包](https://www.debian.org/doc/debian-policy/ch-source.html)、[GNU 对应源码](https://www.gnu.org/licenses/gpl-3.0.html#section6)。
- `objects/usr/lib/zxs-smb-runtime/debian13-amd64/{sbin/smbd,bin/zxs-smb-storage}` 是本次构建的两个角色。收集器用 objcopy 去调试信息，用 readelf 核 amd64 ELF、解释器、每件的 NEEDED/搜索路径和库 SONAME，仅从本次 Debian 编译 staging 的标准 multiarch/专用 Samba 目录带走实际可达的**重建库**；绝不在构建者宿主全盘找同名库，不用 ldd。对象目录不是可运行的根视图。
- `UNSIGNED-OBJECTS.json` 绑定原输入和每件结果的路径/字节数/摘要；同时列出**尚未供应**的解释器与没有在上述重建库中解析的外部 SONAME（只是直接出边，不是外部的完整传递闭包）。`INCOMPLETE` 存在就不是一对完整中间件；失败不续用或覆盖半成品。没有任何一步签章、向 `/etc` 出具发行批准、注册 systemd 或启用 SMB。
- **仍需单独完成：** 从经认证的 Debian 来源锁定所有剩余 ELF、运行时动态加载/NSS/字符转换等实际需要的资源，保留其准确对应源与许可，核不含全机 Samba/后备账号、编译器或不相干入口；独立准备两套正确的只读根及不可换页临时状态。只有最终完整发行在隔离机双构建、验签、全依赖/不借宿主的冷启及正常/异常停机和真实业务验证后，发行者才可签这些**最终**字节，站点另行批准。不要将仅两个程序的 SHA、构建环境摘要或本配方的 Git 提交当作 [主管](../../docs/smb-host.md) 的信任正本。该固定版本仍拒未完成的线上认证和 FILE 路径，制作中间件不提供登录能力。
