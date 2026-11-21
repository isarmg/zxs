# Debian 13 amd64：独立的离线原生账号维护角色

仅是**发行端的源码配方**；构建出来的任何程序都不等于站点准许，也未执行或验证真机。它不依赖 ZXS 源码或服务。唯一原生源码为经 Debian 上游签名链核验的 Samba `2:4.22.11+dfsg-0+deb13u1` 及其自带的 libtdb 1.4.13，和本项目固定[覆盖层](../../resources/samba-native/)。以已应用 Debian quilt 的原源重建，Waf 目标只连 `tdb` 与上游 `tdb-wrap3` 的准确实现；不拾取工具链宿主 libtdb、stock pdbedit、任何宿主账户或给 smbd 装动态插件。

在独立的非 root、离线且已独立核过 Debian 镜像上游签名的发行环境中，按同[固定 Debian 原档、开发者 keyring 和 BUILD-ENV-2](online-smb.md)准备两套只读镜像及完整工具链/许可材料。额外驱动只接由发行者独立验证的 [Debian Patchelf 0.18.0-1.4](https://packages.debian.org/trixie/patchelf) 及实际文件 SHA。覆盖层的身份按配方列序逐文件生成 `<固定名字><空格><完整 SHA><LF>`，再核整个 NATIVE-INPUTS 的独立摘要。不得在同一待验环境现算现批。安全索引过期停止，需要重核新锁，不禁用认证。

在另行批准实际组包时，`native_account_build.py` 固定在自己新建的 `/build/helper-native-account-v7` 工作，选从本次真实重建和经 Debian 签名索引核验的 .deb 解出的依赖；拒决任何跑来的其他来源。按 [Patchelf 的真实 NEEDED 及 GNU 版本文件名更改](https://github.com/NixOS/patchelf/blob/0.18.0/src/patchelf.cc#L1654-L1709) 把所有依赖边固定到 helper 私有路径，移除搜索回退，实核相同基准 glibc、逐件明确 DT_NEEDED/SONAME 及四个定义入口。唯一基准 libc/ELF 加载器是同一 helper 包已有的私有两件，第三方账号角色只携额外的 Samba 等 DSO。缺件、其他解释器、未固定版本路径、可用 trace 版本的 TDB、混入宿主库即拒。未作实际加载/符号重定位/独立双构建，打包源码不证明兼容。

`role/` 与 `sources/` 同时保留原件、选中 .deb 的 Debian 签名来源链/对应完整源、版权、Waf 原件与增补、本次构建依赖源码许可和记录。发行者独立复核对应源码与许可证完整性、检查受限进程实际加载能力后，才准将 `role/`、`ELF-INDEX-1` 摘要、对应源码归档和其摘要作为普通 helper 组包的额外输入。安装包不自动写站点许可、不调用 setup-pair、不建目录、不启服务。宿主后续按[独立本地策略与维护操作的现场批准](../../docs/samba-domain.md#独立发行与站点策略)核定准确范围和制品摘要；密验操作不从文件名或存在性取得权限。
