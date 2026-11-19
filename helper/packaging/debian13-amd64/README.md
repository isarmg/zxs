# Debian 13 amd64：helper 与私有磁盘工具离线发行目录

这是**发行端的组包配置**，不是已经构建、签名、审计通过或发布的安装包。首批打包 bubblewrap 0.12、dosfstools 4.2 的 `fsck.fat`、exfatprogs 1.2.9 的 `fsck.exfat` 以及实际所需的运行库。能调用的是受限 FAT32 副本只读检查；包含 exFAT 程序不代表已经实现或获准调用。没有硬盘/分区修复入口。

## 受信输入

1. 使用独立、受保护、未挂载业务介质的 Debian 13 amd64 发行机与普通构建账户；具备受信的 Python 3.11+、Git、Debian archive keyring、gpgv、dpkg-deb、binutils/readelf、patchelf。只从干净的 helper 源码生成；先按[构建核对表](../../RELEASE.md)从指定源和 Cargo.lock 独立构建、取得受信 SHA-256 与构建记录。本脚本不替这一记录背书，不执行构建出的 helper。
2. 预先提供只读目录：`debian/`、`debian-security/` 各保留标准 `dists/.../InRelease`、`main/binary-amd64/Packages.xz`、`main/source/Sources.xz` 及所选包的 `pool/...` 原路径。另有 `rust/名称-版本.crate`，与本工程 Cargo.lock 的每份归档一一对应。[锁](lock.json)记录了本次实际对照的两份 InRelease 摘要和每个准确二进制包/版本/摘要，不用“latest”。索引日期分别是 2026-09-12（trixie）和 2026-11-19（trixie-security）。过期后仍可另行审读保存的旧证据，本组包器不会用已过期的安全索引再发新候选；每次新发行须复核仍在有效期内的安全索引及修补版，过期或已被修订时须重新评审锁，不能把一个固定哈希当永久安全保证。
3. 构建器用本机独立维护的 Debian 发行公钥验证 InRelease，然后核**签名内容里的**索引散列/大小，再从索引核每个原 .deb 及 Sources 中匹配版本的完整 .dsc、上游源码包、Debian 补丁/打包源码和其他列出的源文件。不会下载文件、运行 .deb 的脚本、执行任何随包 ELF，或将未知源码换成首页链接。缺一项即失败，不是“精简发行”。

在上述输入已核、获准实际打包时才执行：
```sh
python3 packaging/debian13-amd64/build.py \
  --first-release \
  --inputs /release/mirror --helper /release/build/helper \
  --helper-sha256 <已核原二进制的64位十六进制> --source-commit <准确完整提交> \
  --stdlib-notices /release/build/rust-stdlib-and-linked-notices \
  --stdlib-notices-sha256 <与工具链和实际链接输入对应的材料摘要> \
  --output /release/candidates/helper-toolpack-1
```

## 实际形成的东西

- 一份 `local-ops-helper_*_amd64.deb` 包含 `/usr/bin/helper` 与单一受保护前缀 `/usr/lib/local-ops-helper/disk-tools-v1`。用户安装经核发行物后无需另行安装 fsck 工具，也不依赖 PATH。没有运行期下载器或自动安装包管理器。仅有明确允许的普通文件：三个上游程序、GNU ELF 解释器、libc、libcap、libselinux、PCRE2 与 libgcc_s。解包不继承包条目的符号链接、SUID、capabilities、设备文件和脚本；不捆绑 mkfs、smartd、e2scrub、udev/sudo 配置、内核模块或 systemd 单元。
- 实际检查器与所有库的字节不作改写。仅对发行副本的 helper 和启动器用标准 patchelf 重定位 **ELF 解释器及 DT_RPATH** 到这一受保护的绝对目录，并设置 NODEFLIB。构建经 readelf 检查所有直接/传递 ELF NEEDED 均有对应私有库，检查器只需 libc；不调用 ldd。动态装载仍以受信宿主内核/安装根和启动环境为前提，不能用签名证明整个宿主可信或无同权限篡改。FAT32 执行器在实际开动前重新核其所需的八份固定最终字节及路径；只把工具、解释器和 libc 交给检查器，不交出完整工具包目录。包内 BUNDLE.json 记录源码、版本、输入/输出哈希、重定位和依赖。
- 同一发行目录中提供签名索引、原始二进制包、准确对应源码、原补丁、本项目源归档（含组包脚本）、锁内 Rust 原档与 SHA256SUMS。安装包里随附 Debian 原版权/例外与常用许可全文、锁内每份 Rust 原包的署名/许可，以及根据精确工具链、实际链接输入另行审过并带独立摘要的声明；libgcc-s1 的 Debian 版权取同版 gcc-14-base 的原件。GPL/LGPL 等部件仍有各自义务，不能只交付 .deb、短许可名或仅给上游链接。[Debian 源包规范](https://www.debian.org/doc/debian-policy/ch-source.html) · [GNU 对应源码说明](https://www.gnu.org/licenses/gpl-faq.en.html#GPLInProprietarySystem)。
- 输出只在一个全新目录产生。有 `INCOMPLETE` 即禁止使用；完成也仅是未签候选。正式发行前须独立核构建过程、ABI、源码/许可证和归档、安装/冲突处理、已签清单及带外可信签名公钥，在干净受控 Debian 13 环境实测再发布；本次均未执行。不给 Web 安装任意系统软件或 root 身份。

其它格式分开接：e2fsprogs/ext4、xfsprogs/XFS、ntfs-3g/NTFS、btrfs-progs/Btrfs 及 smartmontools 尚未随本版配方提供。OpenZFS 不随应用安装模块或启动池；只有宿主已经加载并验证匹配内核/用户态、可严格限定只读状态查询且不会触发 modprobe 时才可另行开放。`zpool status` 不是通用镜像检查，当前不调用。产品无须用这些文件系统保存自己的数据。

## 私有运行库的安全更新

宿主 apt 升级不会替换本私有目录中的加载器、libc 或其它库。复核 Debian 公告与新签名索引，更新准确包、对应原源码/版权、ELF 闭包和锁内的 `debian_version`，重新发行整个 helper 包，不得同版本替换内容。更新时必须传 `--previous-version`（取自上一份经独立验签的正式发行记录）；组包器按 Debian 的版本比较规则拒绝重复或倒退。只有核实从无发布才可用 `--first-release`。两项不可均给，也不可均省略；缺失、空值、错误文法及相同/倒退版本都不产生候选。这不替代对真实签名发行历史的审核。

未来获准实测后：发行操作不支持边跑旧 worker 边原地换私有目录：先停止接收检查并等现有 worker 退出；前后摘要复核不是和 dpkg 整组换文件的原子锁。在隔离 Debian 13 amd64 上升级，核新签名及 BUNDLE 中每个实际文件摘要；启动新 helper 进程，证明固定启动器、加载器、私有库和检查器都符合新摘要，再对独立已知的只读副本验证。若槽在单次观察期间变化，结果不作已验证使用；不能把此事后检查当作并发升级的保证。没有签名、升级/重新核验及上游公告复核，不称已经修补。
