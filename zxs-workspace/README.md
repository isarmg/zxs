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
- **新的 `commit`（包括 `--fork`）**：停止工作目录的所有写入者。将 `STATE` 放在本人独占、稳定且持久的本机目录中；工作树父目录和 `STATE` 都不能被同步、搬移或重新挂载。其他合作客户端可以竞争归档槽位，但不能动别人的临时上传文件。模式 0700 和 `--writers-stopped` 不是对其他进程的锁。
- 新提交的冻结阶段仅接纳能证明原挂载的 Linux ext4/XFS（当前未接纳子目录挂载），并要求所选 `STATE` 文件系统实际提供不可链接的匿名临时文件。工作内容只写入一个无名称文件，再从同一句柄核验并打包；正常退出、失败或进程结束都无需按名字递归清理。峰值仍需一份未压缩工作内容加一份本地归档的空间，关闭句柄不是安全擦除。单独创建的具名候选 `STATE/<原操作>.zxs` 如在登记前失败，可能只写了一部分；出错时会尽可能给出该名称，不会自动删除、认领或重试。已有 `freeze-*` 目录也不会被扫描或清理。无法证明这些条件时，在打开 SQLite 前拒绝不支持的 `STATE`；不退回具名暂存目录。不影响只读浏览或原提交的 `resume`。这项改动未运行，macOS / Windows 未提供等效的新提交冻结实现。
- 先在本地生成、关闭并验证完整 ZIP，记录原操作，再上传临时同级文件，关闭、复读并验证。只有底层支持 **原生不可覆盖 rename** 和目录同步时才发表。分支同一代只能有一个提交；冲突保留原包和私有候选，不自动覆盖或分叉。失败后的 `.partial` 不当成提交，也不自动清理。
- 断线后用 `status` 查看原 `pending_operation`，再执行 `python3 -m zipworkspace resume STATE OPERATION`。客户端从本次已打开的原存储目录读取准确的代际槽位，在同一归档句柄核验整包 SHA-256、节点和全部成员。文件及目录同步后须仍是刚核验的文件和目录，才能确认成功；否则不清除待决操作。若本次改名明确成功，还须是刚上传的文件实体。重新打开时只凭已登记的原标记和整包摘要核对，不把同名存储认作先前持有的目录：必须先由操作者确认仍是原存储。找不到原证据不凭当前工作树重新签发。
- `branches ARCHIVE STORE` 仅列本目录仓库的分支，区分观察到的最高代与祖先完整的分支头。可用 `checkout-branch ARCHIVE STORE WORKTREE STATE BRANCH_ID` 或 `checkout-node ARCHIVE STORE WORKTREE STATE NODE_ID` 固定选择；不会根据文件日期或后来出现的包重选。在本机保存原包之前发现来源被替换时不发表目标目录。
- `changes STATE` 对照原提交显示真实新增、修改和删除的普通路径。工作树只能由其登记的原目录继续提交；同名同内容的新目录不能冒充。
- `hide STATE --writers-stopped` 只移动已提交且干净的原工作树，归档不变。恢复用 `restore STATE --writers-stopped`；原名已被占用时拒绝覆盖，也不删除任何一方。这里的“隐藏”不是权限隔离。断电中断后用 `status` 显示的原操作执行 `resume-visibility STATE OPERATION --writers-stopped`。
- 同代竞争留下的未发布候选可用 `fork-pending STATE OPERATION` 明确另起分支。按已登记的完整 SHA-256 和节点核验原候选；从同一打开的文件复制，在独占创建的新 ZIP 句柄中改写节点标记，不重新按路径打开文件修改。冲突后的工作树编辑不包含在内，原件保留。若原件或新名称变化，拒绝把新包登记为成功；保留可能未登记的候选，不自动删除。已有分叉的重试仍只核对原后继操作。此保护不代替稳定的原归档存储与私有状态目录。
- 只读核验普通未加密 Store ZIP：`python3 -m zipworkspace import-zip archive.zip NEW_DIRECTORY --preview`。输出原包 SHA-256、整树摘要、条目数及拟用目标；不创建该目录。命令不带 `--preview` 会明确拒绝写入。拒绝压缩或加密成员、链接、设备、重复路径、路径穿越和跨平台别名。
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

**与本机文件夹比较**：打开快照后，选择一个已有文件夹。窗口按相对路径列出本机新增、修改（含文件与目录互换）、缺少的条目，保留空目录；同大小的文件也按实际内容摘要比较。结果是两次目录核查取得的一次观察，不是原子快照或已登记工作树的状态。若发现来源被替换、成员改变、符号链接或无法核验的条目，不显示部分结果。普通文件的硬链接按各自可见名称比较。应用不登记、不解压、不写入所选内容；读取可能由文件系统自动更新访问时间。不具备所需目录句柄能力的平台不提供比较。

Windows 比较器已写入源码，未在 Windows 实机验证。只尝试带盘符、可核验为固定本地 NTFS 卷的目录。它持有所选目录和逐级打开的成员，比较普通文件的默认数据流；不比较 NTFS 附加流、ACL 或其他系统元数据。重解析点、脱机或需召回的文件、网络盘、占用冲突和无法证明的身份均停止，不改用路径拼接扫描。它没有写入、签出或挂载能力。

**核验普通 ZIP** 是独立只读入口。选择普通 Store `.zip`、已有父目录和拟用的新目录名，可查看完整原包 SHA-256、条目数和文件字节数。当前所有目的地都不开放提取：不可覆盖改名能保护目标名称，却不能证明被改名的仍是应用创建的暂存目录。仅凭目录权限也不能排除其他程序替换暂存条目。应用不创建文件，不清理既有目录或失败留下的材料。

桌面登记、签出、提交/分叉、原操作恢复及隐藏/恢复尚未提供。平台没有可核验的本机写入能力时仅提供核验，不将其称为 Windows 写入、真实 SMB 或已验收的桌面工作流。

## 平台与发行状态

- **已验证：** Linux 上的 Python 3.12 源码命令行。`checkout` 写入普通本机目录，不会创建挂载点。ZIP64 已用包含 65,536 个条目的归档验证，单个 4 GiB 文件不在这项验证范围内。
- **尚未发行：** 已有可选的桌面图形界面源码，未在 Linux、macOS、Windows 实际启动验收、签名或打包；均无虚拟磁盘挂载发行版。iOS、Android 均无移动客户端。
- **网络存储：** 真实 SMB 共享的不可覆盖发布和断线恢复尚待验证，不能将本机测试当作共享设备的保证。详见[归档校验和平台范围](docs/design.md)。
