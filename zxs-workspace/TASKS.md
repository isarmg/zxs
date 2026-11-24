# ZIP Workspace 交付状态

本页只记录已有证据和待验门槛。[操作入口](README.md)与[架构](ARCHITECTURE.md)分开。源码、托管环境和正式发行是不同结果。

## 已有证据

| 范围 | 已证据化的结果 |
| --- | --- |
| Linux CLI、Qt | [CI通过] [托管本机文件系统与虚拟 X11](https://github.com/isarmg/zxs/actions/runs/45322700954/job/138204775296)；不等于普通用户的实机挂载或发行。 |
| Linux 全量远程传输 | [CI通过] [隔离客机的真实 TLS/HTTP/2 整文件写入、原号查询和读回阶段](https://github.com/isarmg/zxs/actions/runs/45322511258/job/138204176887)。后续 FUSE 失败，缺盘重挂未执行。 |
| Windows 只读桌面 | [CI通过] [Windows Server x64 的 Qt 与 NTFS](https://github.com/isarmg/zxs/actions/runs/45322700879/job/138204775250)。不是 Windows 11 普通用户、写入、WinFsp 或发布资格。 |
| Android Java | [CI通过] [发行 Java 源码编译](https://github.com/isarmg/zxs/actions/runs/45299654390/job/138162198170)；[后续模拟器作业未通过](https://github.com/isarmg/zxs/actions/runs/45322516885/job/138204195371)。 |
| iOS 传输 | [CI通过] [独立传输编解码和回执步骤](https://github.com/isarmg/zxs/actions/runs/45322522592/job/138204214507)；同一作业后续的应用模拟器阶段失败。 |

## 待验门槛

| 范围 | 下一次须证明 |
| --- | --- |
| Linux 本地工作树与只读挂载 | [仅静态] 核对真正 ext4/XFS 上的不覆盖、原身份、热日志与中断续作；另验真实 FUSE 权限、跨块/ZIP64、拒写和正常卸载。 |
| Linux 旧远程编辑 | [仅静态] [Linux 编辑与提交](docs/guides/linux-remote-edit.md)需实际普通账号验接入原卷、停写和卸载，再原号显式提交；回环协议通过不能替代整个编辑流程。 |
| Windows 另存、可写卷 | [仅静态] 真实普通账号 NTFS、来源、ACL、晚占名不覆盖和真实驱动回执；[旧远程编辑](docs/invariants/windows-remote-edit.md)与[发布候选](packaging/windows-amd64/NATIVE-ACCEPTANCE.md)各有门槛，默认不启用 WinFsp 写入。 |
| macOS 原生另存 | [仅静态] 现有[托管 macOS 作业未通过](https://github.com/isarmg/zxs/actions/runs/45322700879/job/138204775505)；[更早真实来源测试](https://github.com/isarmg/zxs/actions/runs/45298114940/job/138123553971)只读回隔离类型、未读回原 URL。真实来源门槛不放宽；另行显式同意的路线仍无前台签名应用验收。 |
| macOS、Windows 原生卷 | [仅静态] 只读数据面、正常卸载和显式上传独立核验。FSKit [SDK 合约](macos-fskit/target/QUALIFICATION.md)与真实签名、授权及挂载分开；WinFsp 默认发行不开放可写卷。见 [macOS 远程编辑](docs/invariants/macos-remote-edit.md)与[固定快照](docs/invariants/macos-snapshot-volume.md)。 |
| 移动应用 | [仅静态] 需各自真机、系统文档授权、前后台取消、原号恢复和签名安装；[Android](android/RELEASE.md)、[iOS](ios/RELEASE.md)分别验收。 |
| 桌面托管旧工作树 | [设计中] 未开放按显示路径重开既有 STATE；须先证明本机持久登记、原数据库与热日志。不是新 Rust 客户端的持久状态。 |

尚无已验收的签名桌面或移动安装包。[托管桌面资格流程](docs/guides/hosted-desktop-qualification.md)列出已证与未证边界。

旧的逐项验收条件保存在[改写前的完整交付清单](https://github.com/isarmg/zxs/blob/85a8988c386baad1305f4f81ce2c57786df6e436/zxs-workspace/TASKS.md)。新 Rust 客户端的证据只归[其独立项目](../zxs-core/TASKS.md)，不为本页功能背书。
