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
Flush/Close 明确返回失败。Close 使用原 detached 保全路径：同 spool 正文实体、
原 workspace/operation、原预算 charge 保留。它不是共享手动稿，不允许冷启动凭此
本地记录认领或自动发表；受权恢复该保全材料仍是后续功能。真正新开的 Manual 句柄
继续通过 `file.draft.write`，其显式提交仍只由原受权入口决定。

无变化的只读/清洁 Flush、已成功的永久精确重试保持原行为。实际 VFS 必须先
PrepareWrite 再修改 FD 并报告 Dirty；绕开协议自行修改裸 FD 不在本片的新增保证中。
断连未保存材料、失败 Close 的已同步材料、已成功共享稿在验收中不得混称。

## 真实验收源码

新增 Server `--lib --features core` 两个精确测试，均使用实际临时池、随机测试凭据、
真实 HTTP Router、实际 Unix socket/SCM_RIGHTS 与真实 Engine，不使用假 Backend 或 receipt。

1. `engine_backend::automatic_mode_tests::real_http_mode_switch_fences_old_auto_rpc_and_retains_undecided_bytes_across_cold`
   - 已有 preparation hook 卡在真实 Auto 准备阶段，HTTP 切换 Busy；放行后原 Auto 成功。
   - 再有未决定字节时相同 HTTP 切换成功。原 Published 请求可精确重放且不增序。
   - 旧 PrepareWrite/Flush/Close 被拒；正文 inode/bytes 与预算完整相等，历史/HEAD 不变。
   - 另一 FILE 仍正常写；新 Manual 句柄真实发表共享稿，冷开不认领旧 Auto 保全材料。
2. `engine_backend::automatic_mode_tests::accepted_auto_rpc_save_cold_completes_before_mode_switch_and_replays_only_original_request`
   - 真实 RPC Flush 在原 `after_accept_1` 切点返回未知结果；HTTP 切换仍拒。
   - 重复 Flush/Close 不换原请求；冷开只完成原 native 决定。
   - 切换手动后同原请求仍精确返回原收据，不新增版本/序号，旧保全正文和预算不删除。

在原 `ordinary-smb-manual-draft` group 追加两项 exact（原单测 1 → 3）。保留旧 Manual RPC/HTTP、
丢回执测试及所有已映射 engine source 覆盖，不放宽固定数量或 archived workflow。
未本地构建、执行测试或运行 selector；源码和定向计划不是通过声明。实际 Samba/Windows
与服务硬杀验收仍不由这两个 socket/HTTP 测试代替。
