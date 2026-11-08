# 当前 ZXS 二进制与辅助工具的生命周期互斥

本片是首次初始化、凭据重置和现用配置激活之前的安全前置，**不是这些操作已经完成**。仅写入生产源码及未运行的测试源码；按本轮指示没有构建、测试、审查、CI 或真实主机操作。

## 当前可执行程序的强制入口

当前 `zxs` 可执行程序不再只依赖操作人声称“已停服”。它在创建 Tokio runtime、打开管理状态/Engine 或绑定监听端口前，对固定 `/zxs` 目录 inode 取得非阻塞 OS 排他锁。目录必须已存在、root 所有、不可被组或其他用户写，路径拒绝链接；缺少、权限不符、争用或必要内核能力不可用均清楚退出，提示手工运行 helper，不偷偷创建管理状态。

租约 owner 放在同步 `main`，位于 `run_with_runtime` 之外。后者返回意味着 Tokio 已结束并等待阻塞池；启动失败、HTTP 退出或信号结束也不会提前释放租约。不会等待一个超时后强杀持久工作并假称退出成功。退出前重核根路径身份；根目录存在不是“系统已经初始化”的标志。

这是当前产品**单主机单 daemon**的固定入口围栏，不提供通过 CLI 自选另一锁路径绕开它的参数。`platform_io::service_lease::ServiceLease` 是共用的目录 fd/所有权/OS 锁能力。通用 `acquire` 支持明确目录与 owner 政策，但生产二进制使用固定 `host()`；不创建、删除或轮换锁文件。

## 辅助工具采用范围

- 打开 Engine 的 `restore-system`、`recovery-preview`、已确认的 `recovery-commit`、`package export` / `package restore` 与 `scrub`，在首次 Engine I/O 前持有同一 `/zxs` 租约，直到同步命令及其 owner 全部结束。即使命令名字含 preview/export，打开 Engine 也可能重放持久工作，不能当作纯读绕开互斥。
- `deployment preview/prepare` 本来就持有 `/zxs` 稳定目录锁，现在与当前 daemon 的同一 inode 协议直接互斥。仅在该操作期间持有，HTTP 回执不赋予以后操作权限。
- 控制台自身不一直占锁，纯独立盘检查/下载、配置草稿、历史包检查、物理 inventory 不因此取得主服务已停的证明。状态接口的 `service_exclusion_established=false` 仍正确：一个空闲控制台不是停服屏障。
- 只增不换的程序首装不覆盖既有执行文件；完整修复/激活将另行要求租约。未来新写入口必须显式持有 guard，不能把一次曾成功拿锁当成永久授权。

## `/zxs` 首次根目录准备

独立 Web 增加“准备安装根与运行互斥”，CLI 为 `host preview` / `host prepare --request-digest <摘要> --confirm-host-root-only`。

root 操作人先看固定目的地、root UID、0755、父目录设备/inode，再明确确认 `prepare-host-root-only`。仅准备该根目录，**不创建 Unix/Samba/管理员账户、不初始化数据库或磁盘、不修改 systemd/SMB、不停止或启动服务**。不存在时在 `/` 的确定 pending 名内创建空目录，核验为空并持有未来 daemon 的 inode 锁，同步后 `RENAME_NOREPLACE` 发布为 `/zxs`，同步父目录后释放。非空 pending、链接、不同权限、冲突和不确定发布均保留证据，不强行清理。

已有安全 `/zxs` 仅核验身份/权限并同步，不修改其内容；已有业务数据不会被重置。准确空 pending 且权限已完成者可原请求重试；在初始 mkdir/权限设置之间中断的 0700 pending 需要明确调查，不自动 chmod 不明既有目录。目标与 pending 同时存在也拒绝猜测。

此准备结果明确 `system_initialized=false`、`service_stopped_or_started=false` 和 `service_exclusion_held_after_response=false`。请求结束没有持久的“已停服票据”，只能由下一操作重新实际取得租约。

## 仍未涵盖的边界

1. 老二进制、不使用当前可执行入口的库嵌入者、Samba/passdb、独立设备写者和第三方进程没有自动加入此协议。升级与维修前必须明确停止这些写者；本片没有通过 `systemctl is-active` 或进程名扫描推断它们已停。
2. root 管理员可换挂载或重绑目录。租约保持的是 inode，不能阻止 root 绕过协议；活动期间不能重绑 `/zxs`。关键调用点可 `recheck`，不把旧 fd 当新名字永久证明。
3. `/zxs` 准备不是完整首次初始化准入。[普通首启前置门禁](helper-normal-start-prerequisites.md) 已在源码接入：现有根、配置和当前首次激活证明不满足则不启动 runtime。helper 已有管理账户/首批存储及服务身份工作进程，但全路径“已正式纳入且一致”、完整安装交接、SMB passdb 与 Samba 停服围栏仍未完成；不先删除未迁完的共用恢复入口。
4. 未运行真实 daemon/辅助工具争用、进程信号排空、断电或目标部署验收。新增单元测试源码描述权限/链接/争用/身份更换、根目录重试及 Engine 路由选择，不代表已经通过。
