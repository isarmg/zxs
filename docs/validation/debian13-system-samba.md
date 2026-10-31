# Debian 13 系统 Samba 配套模块验收

## 当前明确目标与证据边界

目标仅为 Debian 13（trixie）amd64。当前候选固定 apt `samba` 源版本
`2:4.22.11+dfsg-0+deb13u1`、VFS interface 50；完整源/补丁哈希见
[机器可读合同](../../.github/samba-debian-target.json)。源码已从官方 Debian `.dsc` 核对摘要；
这不等同于已经通过实际编译/宿主加载。以同一候选的 `system-samba.json` 成功收据与 GitHub job 终态为准。
原 4.23/interface 51 和 Debian 12/interface 47 均不作为本目标的运行兼容证据。

## 必要门禁

1. [构建容器](../../.github/containers/debian13-build.Dockerfile) 从 Debian 签名 apt 仓库安装精确目标软件及构建依赖；policy-rc.d 禁止安装时自动启动服务。
2. [SDK 构建](../../.github/scripts/smb-sdk.sh) 下载同版本源包，核固定原档和 Debian 补丁 SHA，应用 Debian 补丁并调用原 `debian/rules configure`，保留完整内部布局/特性参数；仅缩小编译目标，不替换配置合同。
3. bindgen 从真实内部头生成布局；执行全部 VFS FFI/异步/所有权测试、编译 warning 门禁、panic=abort 负例与 release 正例。产出独立 `zxs.so` 和 `VFS-BUILD.json`（精确 Git commit/tree、apt/SDK版本、模块 SHA）。
4. [运行容器](../../.github/containers/debian13-runtime.Dockerfile) 只有系统 apt Samba、其通用运行依赖及验证用 Python，没有 compiler/Cargo/SDK。`--network none` 仅允许隔离 loopback，不开放用户网络或主机服务。
5. [实际客户端验收](../../.github/scripts/check-system-samba.py) 以既存隔离 root 身份和一次性独立凭据，在私有临时 passdb 中认证；真实 apt smbd 加载 VFS，SMB3 强制签名/加密，拒绝错误密码和匿名，上传/下载校字节、改名，权威 binding 撤销后必须 ACCESS_DENIED，恢复后删除。真实 EngineBackend 独立核对发布及最终状态；不以 mock RPC 或任意连接错误代替通过。

所有配置/凭据/临时磁盘只在一次性 CI 隔离环境；不创建系统用户，不修改用户主机权限、不安装或启动真实部署服务。
部署侧按当前安装说明配置系统 Samba 单一既有账号和独立 SMB 凭据，禁 guest/force-user；ZXS 与 Samba 密码不静默复用。

## 安全更新

版本固定用于识别本次验证，不是要求用户停止安全升级。apt 更新 Samba 后必须重建匹配模块并重跑上述 gate；
未匹配模块应明确拒绝加载。未来版本只有取得其实际源/补丁、生成绑定并通过同样验收后才能新增支持声明。

实际 Debian 4.22 loader 拒绝包含 `/` 的模块名称；配置使用 `vfs objects = zxs`，模块须位于该宿主标准 VFS 目录。隔离 CI 仅只读挂载当前 artifact 到实际 `smbd -b` 的 `MODULESDIR/vfs/zxs.so` 并核对完整 SHA，不修改宿主安装，也不放宽加载器策略。首轮绝对路径配置造成的 `NT_STATUS_INVALID_PARAMETER` 保留为失败证据；不计为模块已经加载。

撤权验收保持撤权前已经认证的同一 SMB tree，在原 authority 已撤销后执行 `ls`，必须返回 `NT_STATUS_ACCESS_DENIED`。客户端就绪 marker 只在初次 tree connect 后的串行本地命令发布，撤权前不执行其它远程命令，避免复用旧拒绝文本。另记录新 tree 的拒绝：该版本 Samba 将所有 connect-hook 失败统一为 `NT_STATUS_UNSUCCESSFUL`，此通用状态本身不当作权限验收。重新启用后仍须真实删除成功及服务端最终状态核验。

隔离测试把两种拒绝明确分开：临时 AuthStore 用新的唯一 authority 替换原 authority，使已认证旧会话返回 Unauthorized/EACCES；同时关闭 repository binding，保证新的 tree 连接被拒绝。两者不是同一种错误，也不修改生产错误映射。任一时点只有一个临时管理员 authority；不创建、更改 OS 账号。随后重新打开原临时 AuthStore 并恢复 binding，验证真实删除和权威结果。
