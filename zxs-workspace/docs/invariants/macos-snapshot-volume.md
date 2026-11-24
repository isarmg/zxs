# macOS：固定快照的只读数据面

[仅静态] 这里只交 Mac 原生只读数据回调和不创建卷的 SDK 资格源码。没有 Finder/CLI 挂载、自动安装、提权或自动联网；不是远程写入。

## 原快照、目录与读取

- [`selected_callbacks`](../../zipworkspace/macos_snapshot.py)只接持有原只读文件描述符的选择。用独立偏移读，核完整 ZIP、成员和树标记，不按名称重选、不追 HEAD、不解包。
- 仅接内核确认本机已驻留的普通文件；会读原件的线程禁止自动取回占位字节，再恢复原策略。不是对选择器全流程离线的保证。
- 目录查找以快照的 NFC 和大小写为准；面向 Finder 列名用 NFD、续页不漏项。不能凭空产生资源叉或 AppleDouble；不能把视图合成属性当源 ACL。
- 按偏移读取前，复核涉及 1 MiB 已核块；单请求最多 16 MiB。到 EOF 只交尾段或零。原件变、读短或摘要不符，不交部分本次结果。拒绝写、改名和执行。

## 可选资格与发布分开

测试源码持有原 FD；匹配 macFUSE 5.5.0 / mfusepy 3.1.1 的 libfuse2 ABI 夹具只作回调，不创建卷。生产模块不导入绑定。另一份可选 SDK 源例 `tests/qualification/macos_libfuse_session.c` 只走无设备的进程内单会话。

这不是资格通过。先独立固定依赖，之后真实卷另验调用者、Finder/Unicode、权限、缓存/映射、零长/EOF 和只卸载同一原卷。不能自动退到内核后端或借资格流程安装服务。

[原完整接口、ABI 与上游依据](https://github.com/isarmg/zxs/blob/85a8988c386baad1305f4f81ce2c57786df6e436/zxs-workspace/docs/macos-snapshot-volume.md)保留；[项目发行门槛](../../TASKS.md)另见。
