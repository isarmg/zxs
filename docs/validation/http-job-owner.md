# HTTP backend job 集中所有权候选

基线 `0b3e1ef56ab8b499dd7421e2152aacb0624d2bef`（R25 中央冻结修复）。本批为 source-only 候选；下面描述源码设计和测试接线，不表示编译、运行或取消测试通过。

## 修改范围

`http_job::HttpBackendJob<P>` 私有字段持 Backend、AuthStore 强 owner 和实际准入资源。`run` 让许可覆盖整个回调；`run_with_permit` 只供三种下载初始读取把 read permit 转入结果。回调只借用 Backend，结果 `T: Send + 'static`，auth 不外泄。同步提交 blocking job 后返回等待 future，保留原先 timeout 包住已提交工作的语义。一个执行点保留 auth 的词法所有权，无空 Drop、运行时复核或新 trait。

迁移 72 个实际 spawn 站点：

- 普通 backend_slots 63：lib.rs 的 RegisterDisk 拒绝入口及 15 个 typed 读取；file_manager 三处；http_workspace 七处；http_protection 四处；http_plan 三处；backup/default_directory/smb_binding 各两处；archive、comparison、create_branch、create_file_branch、create_repository、directory_scope、directory_scopes、directory_selection、disk_directory、disk_mkdir、disk_write_session、file_merge、file_selection、file_tags、listing、merge_link、namespace_batch、note、operation、read、rename_file_branch、historical_save_plan、purge_plan、split_plan 各一处。
- management_slots 三处：workspace_capacity、storage_retry_job、setup/recovery dispatch。仅移动现有 backend 与 auth Arc，不取得内部 Engine，不改变锁顺序、stop/drain、授权或恢复逻辑。
- transfer 六处：upload_status（read+backend 双许可）、authorize_upload、upload_generation（均使用已有 UploadAdmission/claim）、content、workspace_content、checksums（三处 read permit 转入结果）。上传纯 staging worker 保留已有 UploadAdmission，不额外持 auth。

R24 原 12 处手动 auth owner 已删除，统一由 job 保证。普通消费者移动原 Arc；上传授权阶段跨请求继续使用 state，按需 clone 已有 auth Arc，无新 Arc 分配、锁、哈希、序列化、fsync 或配置。

例外不迁：ready/capabilities 的独立管理许可、已有 auth_job/login/password 和 telemetry_job 的真实强持有、spool 启动盘点、完成授权后的两种内容块 I/O、静态响应、SMB 和关闭 drain。通用 Semaphore/transfer Permit 不增加 auth 字段，返回响应不绑定凭据文件锁。

## R25 接入状态

保留 R25 的 typed-only Backend，旧 generic call、BackendAction 和宏分派均未恢复。三 prepare 及 RegisterDisk 共四个新增站点已迁到统一 job；原 acquire/body/parse 位置未自行重排。共享路由表纳入它们，取消容量和 Weak<AuthStore> 取消测试使用同一表。

已保留中央冻结的三 prepare 外层 `Result<Prepare*, ApiError>` 签名及所有实现，HTTP job 闭包传 `parse(body)`，不在 Backend 前解包；current 身份之后才传播字段错误。中央新增未知字段／身份优先级交叉回归与 LostCommitReply 死字段删除均保留。本批未再修改 Engine 的解包／授权／历史作用域顺序。

## 测试接线

`test/server/unit/backend_admission.rs`：

- 复用 66 条 typed route 表（含三 prepare 和 RegisterDisk），同时用于容量取消和真实 Router owner 取消测试；增加 repository 列表、管理 capacity/retry、upload-status 和三种下载初始读取共 7 条；加上传两个 gate 和 core recovery，共 76 条 Router owner 场景接线。
- 独立覆盖 upload authorization/publication 两个 gate，以及 core 下受保护 recovery dispatch。
- BackendProbe 在 Router 构造后将自己收到的 auth Arc 取出并丢弃，测试外部仅保存 Weak。取消并 await Router future 后记录存活值，先 release gate 再断言；释放判定使用 bounded timeout/yield 等待 Weak 真正消失，不以 callback 内 finished 通知代替。
- 实际 HTTP content/workspace-content/checksums 响应仍未消费时确认 auth 已释放，然后消费正文；另直接测 Content lease 保持 read permit 占用但不保活 auth。
- 单 blocking thread 的可控排队测试：job 已提交但未执行，取消等待后 owner/permit 仍存在，释放 blocker 后真实完成并归还资源。
- helper 正常返回、具体错误和 panic/JoinError 都释放 owner 与 permit。

`test/server/unit/http_auth/tests.rs` 的 storage_retry_job 调用补充真实 auth owner；既有竞争/撤权断言不变。

## 边界与待验收

请求字节、字段、鉴权、当前身份/Engine 线性化、持久化输入、真实 I/O、JoinError 仍在原边界处理；不移动各 handler 的 acquire/parse/current 先后。类型与所有权保证 worker 的资源期限，不增加业务检查。

已执行限于定向 rustfmt、diff whitespace、架构检查及 Python 架构 fixture；本轮未执行 Cargo（包括 metadata）或构建 target；此前不同基线的 metadata 通过不冒充本轮结果。未执行任何 Rust 编译、Cargo test、负载、socket、SDK、浏览器或 CI。最终集成须跑 server/core/no-core、HTTP、全部新增取消与响应释放、既有撤权矩阵及管理恢复/关闭回归；按最终提交绑定结果。
