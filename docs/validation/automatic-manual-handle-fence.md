# 已有自动句柄遇到手动模式切换

## 生产边界

本片只处理已经存在、具备真实 FILE 历史的普通容器。它不实现新容器默认手动、
无首节点容器、增量保存或高频保护调度，不改变池格式、RPC wire 或公开错误码。

- `EngineBackend::authorize_workspace_write` 在原凭据、实际 workspace 与 selection
  同锁范围内取得实际 FileId，再要求仍为 Automatic。不存在于原 workspace 的真正新文件
  不由路径猜身份；原普通首次保存与容器形状门保持。
- `PrepareWrite`、`Dirty` 复用这一个原准入口。Dirty 在拒绝前仍保存其实际变更记录。
- `Flush` 先查询原 operation。已 Published 的原请求继续 native completion；
  已决定但未完成的请求保持 RecoveryRequired，不重新上传、不换 operation。
  仅确定未有原决定的 Auto 请求，在下一次上传前增加同一模式准入。
- 原 detached preparation 的 `commit_preparation` 门不变。已经开始准备时，
  同时来的 HTTP 切模式必须 Busy；准备完成后，原切换请求才可继续。
  原完整 State/slot/config 再验证与物理 Manual 拒绝是最后决定门，未新增重复全量核。

模式切换后，旧 Auto 句柄不能把已经修改但尚未决定的字节再自动发表为节点。
对于仍存活、普通根级 FILE 的已标记 Dirty 工作代，Flush／Close 可在“完全相同的仓库、
路径、条目与 FILE 身份、分支／节点／版本和选择代数，且模式刚由 Automatic 变为
ManualClean”时，通过既有 `file.draft.write` 发布共享手动稿。该命令沿用未决定的操作 ID；
原权限、工作稿代数比较、正文校验及永久收据依然生效。句柄不能继续获得新写额度，须重开。
中间切换过模式、已有其他稿、选择前移或不支持共享稿的文件仍保留原 detached 路径。
本地记录不是可认领稿，不能在冷启动时凭它自动采用或发布。真正新开的 Manual 句柄继续
通过既有手动稿写入路径，显式提交仍只由原受权入口决定。

无变化的只读/清洁 Flush、已成功的永久精确重试保持原行为。实际 VFS 必须先
PrepareWrite 再修改 FD 并报告 Dirty；绕开协议自行修改裸 FD 不在本片的新增保证中。
断连未保存材料、失败 Close 的已同步材料、已成功共享稿在验收中不得混称。

## 真实验收源码

Server `--lib --features core` 三个精确测试，均使用实际临时池、随机测试凭据、
真实 HTTP Router、实际 Unix socket/SCM_RIGHTS 与真实 Engine，不使用假 Backend 或 receipt。

1. `engine_backend::automatic_mode_tests::real_http_mode_switch_fences_old_auto_rpc_and_retains_undecided_bytes_across_cold`
   - 已有 preparation hook 卡在真实 Auto 准备阶段，HTTP 切换 Busy；放行后原 Auto 成功。
   - 再有未决定字节时相同 HTTP 切换成功；额外切换轮次撤销旧代的共享稿准入。原
     Published 请求可精确重放且不增序。
   - 旧 PrepareWrite/Flush/Close 被拒；正文 inode/bytes 与预算完整相等，历史/HEAD 不变。
   - 另一 FILE 仍正常写；新 Manual 句柄真实发表共享稿，冷开不认领旧 Auto 保全材料。
2. `engine_backend::automatic_mode_tests::accepted_auto_rpc_save_cold_completes_before_mode_switch_and_replays_only_original_request`
   - 真实 RPC Flush 在原 `after_accept_1` 切点返回未知结果；HTTP 切换仍拒。
   - 重复 Flush/Close 不换原请求；冷开只完成原 native 决定。
   - 切换手动后同原请求仍精确返回原收据，不新增版本/序号，旧保全正文和预算不删除。

3. `engine_backend::automatic_mode_tests::live_auto_generation_after_one_mode_switch_saves_a_shared_draft_without_history`
   - 真实 Auto 句柄已标记的 Dirty 字节，经一次 HTTP 切换后成功发布共享稿。
   - 旧 PrepareWrite 被拒绝；Flush 精确重试不重复发布。重新打开读到共享稿，
     不可变历史正文与节点数不变，普通冷开可读回共享稿。

另外，`smb_rpc::tests::native_dispatch_tests::live_automatic_save_becomes_manual_draft_through_the_production_dispatcher`
经真实账户、默认共享分派、会话和 Engine 直接执行同一主线，可在不允许 Unix socket
的环境单独运行。该用例不覆盖对端认证、帧传输或 SCM_RIGHTS。

`ordinary-smb-manual-draft` 包含这些精确用例以及原 Manual RPC／HTTP 和丢回执用例。
实际 Samba／Windows、高频自动保护与硬断电验收不能由这些定向测试代替。
