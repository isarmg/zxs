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

运行本项目测试：

```sh
python3 -m unittest discover -s tests -v
```

## 平台与发行状态

- **已验证：** Linux 上的 Python 3.12 源码命令行。`checkout` 写入普通本机目录，不会创建挂载点。ZIP64 已用包含 65,536 个条目的归档验证，单个 4 GiB 文件不在这项验证范围内。
- **尚未发行：** Linux、macOS、Windows 均无桌面图形界面或虚拟磁盘挂载发行版；iOS、Android 均无移动客户端。当前源码不是这些平台可直接安装的签名 Release。
- **网络存储：** 真实 SMB 共享的不可覆盖发布和断线恢复尚待验证，不能将本机测试当作共享设备的保证。详见[归档校验和平台范围](docs/design.md)。
