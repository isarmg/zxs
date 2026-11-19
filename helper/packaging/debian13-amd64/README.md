# Debian 13 amd64：helper、私有磁盘工具及首装 Samba 材料的离线发行目录

这是**发行端的组包配置**，不是已经构建、签名、审计通过或发布的安装包。首批打包 bubblewrap 0.12、dosfstools 4.2 的 `fsck.fat`、exfatprogs 1.2.9 的 `fsck.exfat` 以及实际所需的运行库。能调用的是受限 FAT32 副本只读检查；包含 exFAT 程序不代表已经实现或获准调用。没有硬盘/分区修复入口。本配方也为专用 Samba 实例的**离线初次域供给**收集私有运行材料，绝不安装或启动宿主 smbd。

用于**在线**专用 smbd 和非 root 存储进程的是[另一份离线原生构建配方](online-smb.md)。它不属于下述磁盘工具包或离线建域 Python 胶囊；不会把系统 smbd、普通 Debian 包中的原服务或用户库当作专版。

## 受信输入

1. 使用独立、受保护、未挂载业务介质的 Debian 13 amd64 发行机与普通构建账户；具备受信的 Python 3.11+、Git、Debian archive keyring、gpgv、dpkg-deb、binutils/readelf、patchelf。只从干净的 helper 源码生成；先按[构建核对表](../../RELEASE.md)从指定源和 Cargo.lock 独立构建、取得受信 SHA-256 与构建记录。本脚本不替这一记录背书，不执行构建出的 helper。
2. 预先提供只读目录：`debian/`、`debian-security/` 各保留标准 `dists/.../InRelease`、`main/binary-amd64/Packages.xz`、`main/source/Sources.xz` 及所选包的 `pool/...` 原路径。另有 `rust/名称-版本.crate`，与本工程 Cargo.lock 的每份归档一一对应。[基础锁](lock.json)和[专用域的候选原包锁](samba-packages.json)记录了本次实际对照的两份 InRelease 摘要和每个准确二进制包/版本/摘要，不用“latest”。索引日期分别是 2026-09-12（trixie）和 2026-11-19（trixie-security）。过期后仍可另行审读保存的旧证据，本组包器不会用已过期的安全索引再发新候选；每次新发行须复核仍在有效期内的安全索引及修补版，过期或已被修订时须重新评审锁，不能把一个固定哈希当永久安全保证。
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

- 一份 `local-ops-helper_*_amd64.deb` 包含 `/usr/bin/helper` 与受保护的磁盘工具前缀 `/usr/lib/local-ops-helper/disk-tools-v1`。用户安装经核发行物后无需另行安装 fsck 工具，也不依赖 PATH。没有运行期下载器或自动安装包管理器。该磁盘工具前缀仅含明确允许的普通文件：三个上游程序、GNU ELF 解释器、libc、libcap、libselinux、PCRE2、libgcc_s，以及仅在新账户首设时按固定路径加载的 Libgcrypt 与 libgpg-error。解包不继承包条目的符号链接、SUID、capabilities、设备文件和脚本；不捆绑 mkfs、smartd、e2scrub、udev/sudo 配置、内核模块或 systemd 单元。
- 实际检查器与所有库的字节不作改写。仅对发行副本的 helper 和启动器用标准 patchelf 重定位 **ELF 解释器及 DT_RPATH** 到这一受保护的绝对目录，并设置 NODEFLIB。构建经 readelf 检查所有直接/传递 ELF NEEDED 均有对应私有库，检查器只需 libc；不调用 ldd。动态装载仍以受信宿主内核/安装根和启动环境为前提，不能用签名证明整个宿主可信或无同权限篡改。FAT32 执行器在实际开动前重新核其所需的八份固定最终字节及路径；只把工具、解释器和 libc 交给检查器，不交出完整工具包目录。包内 BUNDLE.json 记录源码、版本、输入/输出哈希、重定位和依赖。
- 同一发行目录中提供签名索引、原始二进制包、准确对应源码、原补丁、本项目源归档（含组包脚本）、锁内 Rust 原档与 SHA256SUMS。安装包里随附 Debian 原版权/例外与常用许可全文、锁内每份 Rust 原包的署名/许可，以及根据精确工具链、实际链接输入另行审过并带独立摘要的声明；libgcc-s1 的 Debian 版权取同版 gcc-14-base 的原件。GPL/LGPL 等部件仍有各自义务，不能只交付 .deb、短许可名或仅给上游链接。[Debian 源包规范](https://www.debian.org/doc/debian-policy/ch-source.html) · [GNU 对应源码说明](https://www.gnu.org/licenses/gpl-faq.en.html#GPLInProprietarySystem)。
- 输出只在一个全新目录产生。有 `INCOMPLETE` 即禁止使用；完成也仅是未签候选。正式发行前须独立核构建过程、ABI、源码/许可证和归档、安装/冲突处理、已签清单及带外可信签名公钥，在干净受控 Debian 13 环境实测再发布；本次均未执行。不给 Web 安装任意系统软件或 root 身份。

其它格式分开接：e2fsprogs/ext4、xfsprogs/XFS、ntfs-3g/NTFS、btrfs-progs/Btrfs 及 smartmontools 尚未随本版配方提供。OpenZFS 不随应用安装模块或启动池；只有宿主已经加载并验证匹配内核/用户态、可严格限定只读状态查询且不会触发 modprobe 时才可另行开放。`zpool status` 不是通用镜像检查，当前不调用。产品无须用这些文件系统保存自己的数据。

## 专用 Samba 供给运行时

这不是面向全机的 Samba 或 Python 安装器。[组装器](samba_capsule.py)只从经发行签名认证的 `2:4.22.11+dfsg-0+deb13u1` 和所锁的 Debian 13 amd64 原包，写入 helper 私有的 `/usr/lib/local-ops-helper/samba-4.22.11-deb13u1-v1/usr`。正式安装后不需临时联网 apt install。只为[受控专实例首装](../../docs/samba-domain.md)预备材料，私有解释器不会作为宿主级命令安装。随包分发的只读模板 `/usr/share/local-ops-helper/initial-live.conf` 只作实例准备来源，不安装于 `/etc` 或 `/var`、不覆盖配置、不启动服务；BUNDLE.json 记录其路径和完整摘要。本配方不提供 smbd、另一个用户密码库、主机注册表/配置、插件搜索回退或开机服务。

- `samba-packages.json` 是候选原包，不是把这些包的全部程序搬进客体。先正向选择准确 CPython 3.13、标准库源码、Samba Python 入口、LDB/TDB/TALLOC 及由 passdb 初始化动态导入的 dcerpc.security、misc、base；字符转换只取固定 libc 模块/声明。标准库是两个已锁定的 Debian 标准库包中受筛选的顶层 Python 文件，以及 encodings、importlib、collections、re、logging 五个具名源码子树，**不是**本次真实导入过的精确集合。排除测试、site 自定义、.pth、subprocess/pty/webbrowser 等不需要的单独入口；这不是语言沙箱。宿主只许内嵌的固定脚本，不能从清单任选脚本/参数。打包时不运行私有解释器“发现”依赖。对实际 ELF 的 NEEDED 逐件跟到原字节，只有**该请求者**声明了受支持的 Samba private 搜索目录才能在那里选其直接依赖；否则只用 Debian 默认 multiarch 目录。保留经验证的原件字节、正常 SONAME（包括 Debian 私库 `.so.0`）；没有宿主 ld cache、LD_LIBRARY_PATH、其他 Python 环境或同名扫描兜底。
- 受限 ASCII 清单 `CAPSULE` 放在 guest 的 `usr` 之外，记录每个落地普通文件的路径、字节数、权限、SHA-256、ELF 依赖与准确供给原包成员。文件行按原路径排序、逐字段用制表符分隔；目录仅允许作为相应文件路径的前缀，不允许空目录或链接。`BUNDLE.json` 绑定该清单摘要。发行目录保留全部锁定输入的准确 Debian 原包、对应源包及补丁。安装包随附实际供给者的原版权/例外；常见的包内版权别名只从已认证的同套文档解析，不能指向宿主。
- 发行目录另生成单行 `SAM-RELEASE`，固定顺序为 `HOST-SAM-RELEASE-1`、CAPSULE 全文 SHA-256、经重定位的最终 helper ELF SHA-256、实际八槽 launcher 配置的 SHA-256，以单个空格隔开并以 LF 结束。安装包**不会**自动把它变成许可。发行者实际测试完整私有包后才签署校验和；宿主管理员通过独立可信渠道核签名、原版本/校验和与包，另将这一份正本作为 `/etc/local-ops-helper/samba-provision.release`（root:root，0600，单链接普通文件）登记。不从已装目录重算自签，不授权建域，更不替代本次前台确认。旧进程排空后才可更换整套软件或更新该记录。
- 这是**组包配方的静态范围**。没有实际组包、签名、发行或运行证据。可靠的启用仍需单独的发行签名、全文件清单与安装实物的相符核验，并在干净冷启动的 Debian 13 机上验证固定隔离环境的实际导入、本次三次上游调用/正确受控退出；缺库或未能证明来源就停，不挂宿主目录补救。清单不能授权对介质建域，更不能证明用户能登录。

资料：[Debian python3-samba 文件清单](https://packages.debian.org/trixie/amd64/python3-samba/filelist)、[python3-ldb 文件清单](https://packages.debian.org/trixie/amd64/python3-ldb/filelist)、[Debian 的 Samba 源文件](https://packages.debian.org/source/trixie/samba)。

## SMB 首设的可选发行材料

另从 Debian 13 amd64 的 [libgcrypt20](https://packages.debian.org/trixie/libgcrypt20) 与 [libgpg-error0](https://packages.debian.org/trixie/libgpg-error0) 固定原包解出两个真正共享库的完整字节。保留原包的对应源码、补丁与版权；不打包配置器、系统服务或启用兼容模式。它们不是普通诊断及原回执查询的启动依赖。交付前应在清洁 Debian 上核全包来源、目标 ELF 形状和实际原库；对缺库、受保护策略和 FIPS 模式分别验证，不以“能调用旧算法”替代政策许可。

输出目录的 `PAIR-RELEASE` 只记 `PAIR-RELEASE-1`、最终 helper、Libgcrypt、libgpg-error 的三个 SHA-256。它列入发行签名材料，**不是**主机准许 NTLMv2 首设的配置。按[专实例首设范围](../../docs/samba-domain.md)须另有管理者预先在此池/实例签出的本机政策。包不会直接安装这份许可，亦不自动开服务。

## 私有运行库的安全更新

宿主 apt 升级不会替换本私有目录中的加载器、libc 或其它库。复核 Debian 公告与新签名索引，更新准确包、对应原源码/版权、ELF 闭包和锁内的 `debian_version`，重新发行整个 helper 包，不得同版本替换内容。更新时必须传 `--previous-version`（取自上一份经独立验签的正式发行记录）；组包器按 Debian 的版本比较规则拒绝重复或倒退。只有核实从无发布才可用 `--first-release`。两项不可均给，也不可均省略；缺失、空值、错误文法及相同/倒退版本都不产生候选。这不替代对真实签名发行历史的审核。

未来获准实测后：发行操作不支持边跑旧 worker 边原地换私有目录：先停止接收检查并等现有 worker 退出；前后摘要复核不是和 dpkg 整组换文件的原子锁。在隔离 Debian 13 amd64 上升级，核新签名及 BUNDLE 中每个实际文件摘要；启动新 helper 进程，证明固定启动器、加载器、私有库和检查器都符合新摘要，再对独立已知的只读副本验证。若槽在单次观察期间变化，结果不作已验证使用；不能把此事后检查当作并发升级的保证。没有签名、升级/重新核验及上游公告复核，不称已经修补。

**业务管理 HTTPS 模板。** 配方只向 `/usr/share/local-ops-helper/web-management/` 复制五份固定、带 BUNDLE 摘要的 0444 输入，不装入 /etc 或 systemd 单元目录。nginx/Python/systemd、另行验签的 ZXS、专用站点身份和证书均不是本包替站点安装的依赖；见[生成与明确激活](../../docs/management-https.md)。

可选的[专用 SMB 上线原件预检](../../docs/smb-host.md)另附在 `/usr/share/local-ops-helper/smb-host/`。发行表登记检查脚本及不可直接启用的单元模板的完整 SHA；不带在线 smbd 或 ZXS 存储程序，不将过往仅供离线建域的包闭包认作在线发行，不安装/激活任何服务。
