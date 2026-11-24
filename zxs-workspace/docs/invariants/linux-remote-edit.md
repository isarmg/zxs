# Linux：旧远程编辑卷的停止和移交

[仅静态] 给维护既有 Workspace FUSE 编辑路径的人。普通操作见[Linux 编辑与提交](../guides/linux-remote-edit.md)；新版 Rust 普通目录不依赖本卷。

## 只操作原件

- 原窗口持有已登记且可见的 WORK/STATE。本人独占的 ext4/XFS 空挂载点已选定，与原 WORK、STATE、STORE 分开；不能跟随链接或跨挂载。
- 协作锁不是同 UID 的强隔离。操作者要停其他写入者；普通文件和权限均须原样核对。挂载提供者不管理服务器容器/标签/分支。
- 单文件只操作原普通内容，命名和正文改动由同一活切点交接，不以路径消失或正文相同猜改名、删除。
- 仅在同一原父中，真实独占新建的未绑定临时正文可替换确切旧文件。两件须已关闭并保留，按已登记意图交换，再核原件和同步；不凭文件名猜替换。

## 只相信同一卷的正常停止

1. 停止准入新写，不提前撤销已开的写句柄。原文件和目录都正常关闭、同步后才可请求同一原助手卸载。
2. 原助手正常退出、内核原挂载项消失、原空目录恢复，且 FUSE 循环与任务全部归还，合起来才是本次正常卸载证明。
3. 本机持久 `RUNNING` 先于可写。异常、迟到或未知名字不自动清理、重挂或接管；只靠表项消失不能收尾。
4. 有改动的正常停写从当次原切点取得只读正文，然后完整封存；逐项读回原 outbox 计划及正文才在本机结清。运行状态结清不代表服务器已确认。
5. 必须另行明确提交，沿原容器、原节点和原号传整份，不直发 WORK FD。网络失答按[原回执](remote-recovery.md)分开处理。

来源：[`linux_edit.py`](../../zipworkspace/linux_edit.py) · [`linux_edit_mount.py`](../../zipworkspace/linux_edit_mount.py) · [`linux_submit.py`](../../zipworkspace/linux_submit.py) · [原完整原型说明](https://github.com/isarmg/zxs/blob/85a8988c386baad1305f4f81ce2c57786df6e436/zxs-workspace/docs/linux-edit-prototype.md)。
