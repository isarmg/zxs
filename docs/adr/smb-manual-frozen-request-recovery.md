# SMB 手动稿原请求冷恢复

## 本片的实际边界

本片新增冻结后的原 manual save 恢复；代码及真实引擎故障 fixtures 已写，尚未通过远端编译或运行。
没有执行本机测试、编译、formatter。普通 shared draft 冷开读取不作为恢复通过证据。

每次 manual flush 在提交前生成独立私有 frozen body，并同步正文和目录，再以 create-only
发布 strict `manual_draft_frozen_v1` 记录。记录保存原调用者 entry、FileId、repository/path、
主体、operation、expected generation、完整 DraftBase 与 hash/size。格式自身限定 manual save，
不会在重启后根据当前 mode 猜测原操作。live FD 不持有 frozen body，后续写不能改原请求正文。
两份正文分别通过现有 SMB spool 预算计费；记录包含在原预算元数据上界内。

新版 `RecoverManualDraft` RPC 及 client 方法区分 query 与显式 resume。先经当前身份授权，
按原主体、typed Save、operation 和 canonical digest 查询既有收据。结果查询不要求原 mode、
base 或 generation 仍当前，且不改状态；无结果时 resume 才校验 frozen body 并调用既有
persist 的当前 caller entry、原来源权限、mode、base、CAS 准入。此路径不调用 workspace commit。
旧 `smb-protocol-work` 不是该恢复格式，解析失败保留原字节，不兼容、不自动迁移。

已发布原请求但 FD 又变化时返回原结果并注明 live bytes 已变化；活 writer 必须核对原结果后
再由 Dirty 生成下一 operation。断连后保留 frozen 记录和正文，服务重启可经同一原 operation
查询/恢复；不会自动重挂接旧句柄，也不把后继 FD 字节当成旧请求的一部分。

## 仍待完成和验收

- helper 已接真实 online-only RPC：`smb-manual-recovery --socket SOCKET --username ACCOUNT
  --operation ORIGINAL_OP` 默认只查；末尾 `resume` 才恢复原请求。它不取离线引擎租约，不打开
  engine/spool，不代入全局 `--principal`。旧离线命令仍保持完整 host lease；此入口尚待远端验收。
- 在第一次冻结尚未完成前断连，仍只有旧 live 工作材料，不能宣称已有完整 frozen request。
- 冻结正文或记录发布中断、占位不完整时保留材料并拒绝覆盖，尚无修复该 partial freeze 的入口。
- 本片不自动清理 frozen 证据。其正文/记录持续计费，需后继显式确认清理策略，不能靠丢弃恢复证据回收空间。
- 已新增四个 fresh 单 primary、0700 根的实际引擎回归：未送请求冷恢复与 FD 后变、丢响应的原请求
  核对与下一 Dirty、discard/mode 更改后只查原结果、无结果且 mode 更改拒写及 legacy 原字节保留。
- 真实 RPC 断线、helper、存储故障注入及完整来源移动组合仍需远端执行证据，不以本片存在冒充通过。
