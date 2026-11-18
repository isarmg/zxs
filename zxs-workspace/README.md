# ZIP Workspace

把一棵目录签出到本机，用普通工具编辑，明确提交时才发布整个目录的新版本。主机或 SMB 共享只需保存普通文件：每个已提交目录快照都是一个 **未加密、Store 模式的标准 ZIP**，扩展名为 `.zxs`，大归档使用 ZIP64。要用普通解压工具取回文件和空目录，可将归档复制为 `.zip`；保留原来的 `.zxs`，之后仍可用命令行签出或查看历史。保存归档的主机或共享目录不需要安装 ZIP Workspace 服务端。

本仓库是独立实现。目录仓库、分支、历史和本机工作树由客户端管理；归档内没有逐文件节点、文件容器、专用文件头或隐藏元数据目录。仓库标记位于 ZIP 规范已有的 archive comment。普通 ZIP 和仓库包有独立的打开入口，不能只靠后缀冒充仓库。

## 先试用

本次源码验证环境：Linux、Python 3.12。项目运行时只用标准库。从已经签出的本仓库运行，无需安装依赖：

```sh
mkdir -p demo/work/empty demo/store demo/states
printf 'hello\n' > demo/work/hello.txt
python3 -m zipworkspace init demo/work demo/store demo/states/first
ARCHIVE=$(
  python3 -m zipworkspace commit demo/states/first --writers-stopped |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["committed_archive"])'
)
```

在同一个终端继续。`$ARCHIVE` 是刚才那次提交返回的归档路径，不用猜文件名或寻找“最新”的包：

```sh
python3 -m zipworkspace inspect "$ARCHIVE"
python3 -m zipworkspace browse "$ARCHIVE" --limit 40
python3 -m zipworkspace checkout "$ARCHIVE" demo/second demo/store demo/states/second
python3 -m zipworkspace history "$ARCHIVE" demo/store
python3 -m zipworkspace branches "$ARCHIVE" demo/store
python3 -m zipworkspace status demo/states/second
# 可选：从选中的目录快照明确分叉，仍是提交整个工作树
python3 -m zipworkspace commit demo/states/second --fork --writers-stopped
```

- `init` 只登记已有目录。工作目录、归档存储和本机私有状态必须互不包含。
- `checkout` 只还原指定快照，不换成较新的包，也不会覆盖现有目录。不同工作树和私有状态相互独立。
- 让工具选一个不冲突的目录名：`checkout-auto ARCHIVE PARENT STORE STATE Project`。它从 `Project`、`Project (2)` 起尝试，已存在的目录一律保留；256 个名字都被占用时停止。
- 签出中断时，运行 `checkout-status STATE` 查看原操作，再运行 `resume-checkout STATE OPERATION`。恢复只接认本机已验证的原快照和原目录；不会重新选择更新的分支或仅因字节相同就采用别人的同名目录。无须联网重新下载已经保存的原包。
- 提交前停止工作目录的所有写入者。本版命令行没有权限冻结其他进程。它会通过不跟随链接的目录描述符复制到私有临时快照，并复核整树身份/变更戳；观察到变化则拒绝，ZIP 打包器不会读正在编辑的目录。
- 先在本地生成、关闭并验证完整 ZIP，记录原操作，再上传临时同级文件，关闭、复读并验证。只有底层支持 **原生不可覆盖 rename** 和目录同步时才发表。分支同一代只能有一个提交；冲突保留原包和私有候选，不自动覆盖或分叉。失败后的 `.partial` 不当成提交，也不自动清理。
- 断线后用 `status` 查看原 `pending_operation`，再执行 `python3 -m zipworkspace resume STATE OPERATION`。已提交的同一操作可以被核对；找不到原证据不凭当前工作树重新签发。
- `branches ARCHIVE STORE` 仅列本目录仓库的分支，区分观察到的最高代与祖先完整的分支头。可用 `checkout-branch ARCHIVE STORE WORKTREE STATE BRANCH_ID` 或 `checkout-node ARCHIVE STORE WORKTREE STATE NODE_ID` 固定选择；不会根据文件日期或后来出现的包重选。在本机保存原包之前发现来源被替换时不发表目标目录。
- `changes STATE` 对照原提交显示真实新增、修改和删除的普通路径。工作树只能由其登记的原目录继续提交；同名同内容的新目录不能冒充。
- `hide STATE --writers-stopped` 只移动已提交且干净的原工作树，归档不变。恢复用 `restore STATE --writers-stopped`；原名已被占用时拒绝覆盖，也不删除任何一方。这里的“隐藏”不是权限隔离。断电中断后用 `status` 显示的原操作执行 `resume-visibility STATE OPERATION --writers-stopped`。
- 同代竞争留下的未发布候选可用 `fork-pending STATE OPERATION` 明确另起分支。分叉采用已冻结的原候选，不包含冲突后写入工作树的新字节；原件继续保留。
- 普通未加密 Store ZIP 可用 `python3 -m zipworkspace import-zip archive.zip NEW_DIRECTORY` 导入，再 `init`。拒绝压缩/加密成员、链接、设备、重复路径、路径穿越和常见跨平台别名。保留普通文件字节、目录层级、精确名称和空目录；不承诺保留 ACL、xattr、硬链接语义或纳秒时间戳。
- 一份归档足以恢复其完整目录，**不等于**拥有所有历史。祖先档案不在当前存储位置时，`history` 明确返回缺失父操作。
- 只看目录不必签出：`browse "$ARCHIVE" "选中的文件夹"` 返回直接子项、文件字节数和 SHA-256，空文件夹也会显示。用上一页的 `next_cursor` 继续同一快照和目录；新快照不会接用旧游标。[只读应用接口](docs/design.md#read-only-archive-browser)同样先核验整个归档，不创建工作树。

运行本项目测试：

```sh
python3 -m unittest discover -s tests -v
```

## 桌面窗口（源码，尚未打包或验证）

当前可用和待验的操作见[本项目交付清单](TASKS.md)。

在单独的 Python 3.12+ 环境中，从本项目目录安装可选依赖 `python -m pip install '.[desktop]'`，再以 `zip-workspace-desktop` 或 `python -m zipworkspace.desktop` 打开窗口。命令行不依赖 Qt。依赖来自 [Qt for Python 的 Essentials 组件](https://doc.qt.io/qtforpython-6/package_details.html)。

选择 **打开 .zxs 快照** 可查看大小、SHA-256 和空目录，双击目录进入、按页前后浏览。选择 **归档存储目录 → 读取分支与历史** 才扫描对应仓库；历史节点和已核验的分支头可以单独打开。缺少祖先时显示缺失的原操作，不把该分支当作完整头。文件选择后被替换会拒绝，不用当前同名文件代替原节点。这个窗口不是虚拟磁盘。

首次浏览会读取并核验全部成员，再显示整树摘要和文件摘要；不会为了第一屏再顺序读取整个 ZIP 计算原包 SHA-256。显式读取分支历史、核对一个已绑定的原节点时仍需原包字节证据。归档很大时完整成员核验仍可能耗时。

**导入普通 ZIP** 是独立入口。选择普通 Store `.zip`、现有父目录和新的目录名，先查看整个原包的 SHA-256、条目数与文件字节数；核验和取消都不写文件。确认后才还原普通文件和空目录，并重新核对最终实物与内容。目标已存在、所选文件或父目录已替换，均不覆盖。结果未知时先核对目标和保留的暂存目录，不自动删除或重试。

此操作不建立工作树、分支或 SQLite 状态。桌面登记、签出、提交/分叉、原操作恢复及隐藏/恢复尚未提供。平台没有可核验的本机写入能力时仅提供核验，不将其称为 Windows 写入、真实 SMB 或已验收的桌面工作流。

## 平台与发行状态

- **已验证：** Linux 上的 Python 3.12 源码命令行。`checkout` 写入普通本机目录，不会创建挂载点。ZIP64 已用包含 65,536 个条目的归档验证，单个 4 GiB 文件不在这项验证范围内。
- **尚未发行：** 已有可选的桌面图形界面源码，未在 Linux、macOS、Windows 实际启动验收、签名或打包；均无虚拟磁盘挂载发行版。iOS、Android 均无移动客户端。
- **网络存储：** 真实 SMB 共享的不可覆盖发布和断线恢复尚待验证，不能将本机测试当作共享设备的保证。详见[归档校验和平台范围](docs/design.md)。
