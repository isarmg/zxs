# ZIP Workspace

在普通目录里编辑，显式提交时才生成整目录的 `.zxs` 快照。快照是未加密、未压缩的标准 ZIP；归档大时使用 ZIP64。取文件可把副本改成 `.zip` 后解压，查看仓库历史请保留原 `.zxs`。改普通 ZIP 的后缀不会把它变成仓库。

本地 ZIP 工作区不需要 ZXS 服务。现有桌面另有「存储容器」页，通过 HTTPS、HTTP/2 操作所选远程文件。两条路径暂时共存。[新版客户端](../zxs-core/README.md)使用普通本机目录和手动提交，不维护本地版本库或虚拟盘，尚未替换现有入口。

## 5 分钟上手

在 Linux 本机 ext4/XFS 上用 Python 3.12+。在 `zxs-workspace/` 按[快速上手](docs/guides/quickstart.md)建一份新示例：

1. `init` 登记示例工作目录、归档目录和私有状态；此时没有版本。
2. 像平常一样编辑文件，停掉写入者后用 `commit --writers-stopped` 保存整棵目录。
3. 用 `browse` 查看返回的快照，或用 `checkout` 签出到新目录。

想用窗口，可按指南装 Qt，启动 `zip-workspace-desktop` 并选择「打开 .zxs 快照」。需要读取远程文件时，按[容器页指南](docs/guides/remote-files.md#连接所选服务器)单独连接服务器；打开容器不提交、不下载正文。

## 空间估算

按 CLI、每代正文 100 GiB、已有基线且不另留散包估算：三处同卷时，首次签出约占 300 GiB，连续两次等大小提交时约占 500、700 GiB。是总占用，不是额外空闲量或实测值；[分卷和中断说明](docs/invariants/local-worktree.md#分卷空间)。

## Linux：把一个固定快照挂为只读目录

[仅静态] 有可选的 FUSE3 命令和窗口入口，只查看所选快照，不写入或跟随分支。需要符合条件的本机空目录；见[使用与卸载](docs/guides/quickstart.md#可选linux-只读挂载)。

## 平台与发行状态

| 范围 | 已核到的证据 |
| --- | --- |
| Linux CLI、Qt | [CI通过] [托管文件系统、源码用例和虚拟 X11 窗口](https://github.com/isarmg/zxs/actions/runs/45322700954)；实机挂载和发行待验。 |
| Windows、macOS | [CI通过] [Windows Server 上的只读 Qt／NTFS 用例](https://github.com/isarmg/zxs/actions/runs/45322700879/job/138204775250)；Windows 11 普通用户场景待验，macOS 同批作业未通过；[具体范围](docs/guides/hosted-desktop-qualification.md)。 |
| Android、iOS | [CI通过] [Android Java 编译](https://github.com/isarmg/zxs/actions/runs/45299654390)和 [iOS 独立传输用例步骤](https://github.com/isarmg/zxs/actions/runs/45322522592/job/138204214507)；模拟器整应用和真机待验。 |
| 原生可写挂载 | [仅静态] Windows WinFsp、macOS FSKit 远程编辑有源码，默认发行未开放。 |
| 新版 Rust 客户端端到端 | [仅静态] [独立项目](../zxs-core/TASKS.md)记录实际验证；现有容器管理入口暂不变。 |

没有已验收的签名桌面或移动端安装包。

## 进一步阅读

- **操作**：[CLI、桌面与只读挂载](docs/guides/quickstart.md) · [远程容器页](docs/guides/remote-files.md) · [Android](android/README.md) · [iOS](ios/README.md)。
- **设计与证据**：[架构](ARCHITECTURE.md) · [归档格式](docs/specs/archive-format.md) · [交付清单](TASKS.md) · [本机安全与恢复](docs/invariants/local-worktree.md)。
- **打包配方**：[Ubuntu](packaging/ubuntu-26.10/README.md) · [Windows](packaging/windows-amd64/README.md) · [macOS](packaging/macos-arm64/README.md)。
- **旧版长说明**：[改写前的完整 README 与原有命令、限制和验收线索](https://github.com/isarmg/zxs/blob/37734daef96e41baafa88f82ebccbaeacc5e7a91/zxs-workspace/README.md)。
