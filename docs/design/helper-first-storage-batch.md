# 辅助工具首批存储：服务身份工作进程与准确原请求

基于 `c4f708df`。本片是首次管理账户之后的真实首批空目录初始化入口，**不是全部首次安装就绪、最终干净启动门禁或新逐盘权威协议切换**。按要求只写源码，未编译、测试、审查或运行 CI；未执行辅助工具、修改真实主机、初始化真实磁盘或更改密码。

## 用户入口与范围

独立 Web 新增 `POST /api/initialization/storage-preview`、`storage-create` 和 `storage-recover-preview`。输入已有本机非 root 服务账户与 `/zxs/data/disks` 下的全部直接子目录，明确确认 `initialize-first-storage-batch`。每个已有空目录初始化为生产模式的独立一级盘，不按顺序推断备份链。已有外来或非空根不能走此入口；这也不是二级以上完整链的正式外来准入。

根/账户布局、已存在的管理意图与管理权威必须符合当前协议。所有直接子项必须在本批选择中，目录必须真实存在且归准确服务 UID/GID、0700；额外、缺失、链接、未知、权限不安全或不合格文件系统均拒绝，不通过跳过条目伪造干净目录。内核文件系统、挂载观察、目录身份与空盘准入继续使用通用 storage_directories；核心生产 Config 校验仍生效，不回退 development。

工具不创建 Unix 账户、挂载点或文件系统，不 mount、格式化、清盘、chown 现有目录。操作者先在工具外准备正确的持久挂载和权限，随后明确选择完整首批。只能在首次管理态使用；既有池不能当作空盘重新初始化。

## 权限与生命周期

Web 仍手动 root、默认 `0.0.0.0:9040`、无登录、明文 HTTP，不注册自启。可达者现在也能明确初始化首批空目录，风险提示同步更新；同源/CSRF/IP Host 不是认证，必须由使用者限制在可信隔离网络。工具不调整网络或防火墙。

root 编排者只保管固定目录计划与私有完成回执，不以 root 身份打开服务拥有的 Engine。它通过 `/proc/self/exe internal-storage-worker` 启动同一个已运行二进制的受限工作进程：固定 argv，无 shell、外部服务程序、PATH 搜索或密码参数；清空环境，设置准确非 root UID/GID，独立进程组。Rust 的 uid 设置清空 supplementary groups，子进程还在系统访问前核对真实/有效 UID/GID 和空 supplementary groups。当前辅助二进制本身必须允许所选服务身份执行；本片不会为执行失败放宽它或主机目录的权限。

工作进程不是 HTTP 监听器，不启动 ZXS 或 Samba。请求和响应经有界管道传递，最高 64 KiB。父进程在失败、客户端断开和正常退出时也等待已准入子进程结束，超长响应只排空而不无限分配。子进程独立进程组避免终端 Ctrl-C 同时中断它；父 Web 收到正常停止信号后排空已有任务。强制杀死子进程/断电仍是崩溃，不冒充协作完成。

root 计划阶段与 service UID 工作阶段各自取得 `/zxs` 实际 OS 生命周期租约。两阶段之间**有释放/重新获取的交接点**，不是一把跨进程永不释放的锁：工作进程开始前重新准入完整上下文，若 daemon 或其他参与者先取得租约则拒绝而不初始化。子进程持有租约、管理凭据 writer 和普通 setup/Engine 系统/磁盘锁直到相关对象结束；子进程退出后 root 重新获得租约并核原布局才写 helper 完成回执。若服务先启动导致最后交接忙，报告原操作可能已完成，不能谎称回滚或换新请求重做。

旧二进制、第三方进程、Samba/passdb 及不参加协议的写入者仍须另外停止。页面确认不是这些进程的 OS 互斥证明。本地服务账户原本即拥有这些私有数据；隐藏 worker 不是给其他 Unix 身份提权的入口，非匹配身份在存储访问前拒绝。

## 原计划、初始化与派生材料

预览绑定固定 host/data/state/management/discovery inode、准确本机账户来源、当前管理凭据代数/会话指纹、所有准确目录和挂载选择，不保存密码。确认后先以 0700/0600 将完整原计划保存在 `/zxs/.zxs-helper-first-storage-<摘要>/plan.json`，再交工作进程。未知 pending、不同正文、错误身份或后来的凭据/会话变化均不替换旧计划。

工作进程重用 `SetupRecovery::activate`：保留当前原生 Config/genesis、成员与事务协议，写 activation intent，消费 held 空目录，初始化 Engine。完成回调执行以下真实步骤：

1. 只接纳最初 genesis 或唯一 `auth.control-state` 后的原始池；只接受相同独立一级根集合，不允许把后来已用过的池当首次初始化重做。
2. 将现有唯一管理员的当前快照发布到 Engine 原生灾备控制材料，去掉会话；只在没有快照的 genesis 写入，已有不同快照拒绝，不覆盖更晚权威。
3. 通过现有 writer 显式构建每个首批盘的 current 与 history SQLite，再由现有观察 API 核对当下来源与历史覆盖。
4. 核对系统 SQLite 的当前 State 记录与 index 状态。首批尚无用户对象，不能据此声称已为未来对象提前创建第三份实体数据库。
5. 这些步骤全部成功后，核心才发布原 activation completion。root 随后写 `/zxs/.zxs-helper-first-storage-complete-<摘要>/receipt.json`，保留真实池 ID、序号及每盘派生记录摘要。

该回执表示原生首批 storage activation 完成，但明确 `complete_installation_ready=false`、`clean_start_gate_installed=false`、`service_started=false`、`samba_credentials_changed=false`、`independent_disk_authority_migrated=false`。磁盘派生库不是每盘独立发布权威，全池现有协议仍不能当作最终可移植性完成。

## 中断与旧请求

恢复预览只关联原 helper 计划、当前账户、目录集合和准确 core activation request，不运行 Engine，不承诺它一定可恢复。真正续作使用新增共享 `complete_activation_exact`，在正常 setup 锁内重复核对原选择/模式，不能以别的 core intent 接续。即使 core completion 已存在，也必须执行本入口的材料核验，不能仅凭 completion 文件跳过必要步骤。普通已有 completion API 保留原语义。

在原意图写下前中断，只能原目录观察仍匹配时再启动；已有 intent 则进入当前原生恢复证明路径。缺少 config/locator、部分 genesis 不满足现有证明、损坏、陌生临时项或后继拓扑不会被猜测修好，保留材料并报错。完成恢复的 Engine open 仍可能进行核心既有恢复/派生维护，不是只读检查；发现后来非首次业务状态会拒绝后续首批发布，本片不做通用修复或撤销。

多文件跨目录初始化不是一个大原子事务。错误可能发生在配置/部分磁盘、凭据灾备或数据库已发布之后；保留同一请求、摘要和证据，不能删除控制文件/磁盘字节再初始化。响应丢失、最后 root lease 交接失败或 helper receipt 未完成时也同样处理。运行中的当前服务若在两阶段之间抢到租约只会使本次操作失败，不是绕过互斥的理由。

## 剩余工作与成本

后继[首启准备门禁](helper-normal-start-prerequisites.md) 已接普通可执行入口，但完整正常启动准入、禁止运行态待接入数据的全路径门禁、运行/log/tmp/socket 目录、依赖与 VFS、systemd/Samba 校验及显式启用、实际 SMB 密码重置、后续添加/注销与外来完整链仍未在本片完成。不删未迁完的现有安全入口。系统第一次“可运行”的总体交接仍需下一阶段，不能用此片替代。

选择上限 32 个，目录准入遇到一个未知项立即拒绝。管道与持久原计划各 64 KiB；Unix 账户来源沿用 4 MiB。工作采用一个受管理 blocking owner、一个序列子进程，初始化/数据库屏障沿用核心，不减少 fsync 或成员要求。坏设备可阻塞，进程诚实等待，不设置会假装取消持久写入的短超时。新增纯输入/绑定、原意图不匹配与完成后回调、HTTP 来源拒绝测试源码，均未运行，不是 root、真实挂载、断电或浏览器验证证据。
