# 快速上手

给第一次用 Workspace 的人。准备 Python 3.12+ 和本机 ext4/XFS，用新示例完成登记、提交、查看和签出。
普通 CLI 不装 Qt、不连服务器。Linux CLI 和虚拟显示中的 Qt 有[托管 CI 证据](https://github.com/isarmg/zxs/actions/runs/45322700954)，挂载与发行待验。

## 从命令行保存和签出

用普通 Linux 账号和 Python 3.12+，在仓库的 `zxs-workspace/` 目录操作。示例要求 `$HOME` 位于本机 ext4/XFS；如不符合，请选自己控制的本机卷。需要 `sqlite3.Connection.serialize()` 的 Python 才能新签出。

先执行首行以确认新示例目录创建成功，再执行后续命令。示例不覆盖已登记的位置。

```sh
DEMO=$(mktemp -d "$HOME/zip-workspace-demo.XXXXXXXX")
mkdir -m 700 "$DEMO/work" "$DEMO/store" "$DEMO/states"
mkdir -m 700 "$DEMO/work/empty"
printf 'hello\n' > "$DEMO/work/hello.txt"
python3 -m zipworkspace init "$DEMO/work" "$DEMO/store" "$DEMO/states/first"
```

现在可用普通编辑器改 `work/`。停止编辑、同步及其他写入后执行提交，将返回的新快照路径存到 `ARCHIVE`：

```sh
ARCHIVE=$(
  python3 -m zipworkspace commit "$DEMO/states/first" --writers-stopped |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["committed_archive"])'
)
```

仍在这个终端查看或签出到尚未存在的新目录：

```sh
python3 -m zipworkspace inspect "$ARCHIVE"
python3 -m zipworkspace browse "$ARCHIVE" --limit 40
python3 -m zipworkspace checkout "$ARCHIVE" "$DEMO/second" "$DEMO/store" "$DEMO/states/second"
python3 -m zipworkspace history "$ARCHIVE" "$DEMO/store"
python3 -m zipworkspace status "$DEMO/states/second"
```

或在 Python 独立环境安装基础包，使用等价的 `zip-workspace` 命令。从源码则可直接用上面的 `python3 -m zipworkspace`。

## 从桌面查看快照

在项目目录，用独立的 Python 3.12+ 环境安装 Qt 入口：

```sh
python -m pip install '.[desktop]'
zip-workspace-desktop
```

点「打开 .zxs 快照」，选定示例归档后按提示只读核验。可浏览文件和空目录，或和另一份选定的快照做只读比较。普通 ZIP 走独立的核验入口。

若要读远程文件，在「存储容器」页另行输入已配置的 HTTPS 地址和账号。需服务端入口支持 HTTP/2，桌面环境需[锁定的传输组件](../../packaging/network-r3/README.md)。查看不上传；[各平台的显式提交与恢复范围](../container-management.md)另见说明。

## 可选：Linux 只读挂载

[仅静态] 仅查看固定快照，不用于编辑或提交。安装项目的 `mount` 可选依赖；系统须已有获准使用的 FUSE3。选本人独占 0700 的本机 ext4/XFS 空目录，使用两个自己选定的绝对路径。

```sh
zip-workspace-mount-ro "$ARCHIVE" "$MOUNTPOINT"
```

其中 `MOUNTPOINT` 是你选定的目录。结束时核对目标、只读类型、唯一来源和挂载 ID，再正常卸载。证据不全不按路径卸载。见[来源核对和卸载命令](https://github.com/isarmg/zxs/blob/37734daef96e41baafa88f82ebccbaeacc5e7a91/zxs-workspace/README.md#linux把一个固定快照挂为只读目录)。

如有中断、冲突或空间问题，先读[工作树安全与恢复](../invariants/local-worktree.md)；不要自动换号或清理原件。
