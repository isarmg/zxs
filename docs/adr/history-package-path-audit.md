# History package 的永久路径审计来源

## 决定

`HistoryManifest.path_audit` 是必填的新格式 map。外层键是本地永久 carrier operation，
内容保存规范化的原 source operation、原 principal 和 typed 目标路径事实投影。
原生 rename／batch 的完整原收据及 request digest 不重写，也不按导出文件切片。
增量投影只依赖实际 operation／principal 写集，不因全局 sequence 改变而重建其他 history。

恢复把原 source 证据放进自身 `history.restore` 永久收据的 `restored_path_audit`。
它不把原 source operation 插入本地 `State.operations`，不冒充一次本地 rename。
FILE 路径读取共用 retained typed 事实；原生事件 visitor 仍不把导入来源报告为本地事件。
再次导出将 carrier 中同一 source 的证据合并，不递归包裹旧 restore 收据。
同一原 OperationId 的 receipt／principal 不同即拒绝，完整相同证据才可共享。

已有 restore 明确继承源 publication sequence 的 high-water mark 后再发布；
因此源 sequence 与导入收据的顺序校验依赖这一明确规则，不是假设两个独立池自然同序。
重复导入相同 history／repository／file 身份仍由原 ObjectCollision 围栏拒绝。

## 端点证明与边界

原 event、operation、principal、before／after 路径保持不变。
copy 只改 typed 目标 file／history／branch／version 投影，并记录 typed copy OperationId
推导链，逐步重算 file／branch／version 身份以验证目标端点。restore 不修改该链。
该链证明身份推导，不声称包含每次 copy 的完整原生收据或完整跨池操作历史。
目标端点另使用原永久 graph ownership verifier，包含冻结 commit 的借用正文闭包。
既有删除／undo 的 CopiedLifecycle.events 及验证保留，不据此声称新增完整删除审计包迁移。

一份原 batch 的所有事实必须具有导出包内的完整目标映射，且保留完整原 request／transfers。
仅复制多文件 rename 中一个文件后导出会明确拒绝，而不是漏掉其他成员或暗中导出包外数据。
上述含路径审计的 batch 中，没有文件端点证明的空目录 transfer 同样拒绝。
含既有路径来源前缀的混合 object_move 保留完整原 payload，但当前不能规范化为可携带证据，
故 portable export，以及以它为来源、需要 capture_mapped 的历史复制／移动明确拒绝。
后者包含再次启用 copy_history 的 object_move，不只是包出口或 copy API 的限制。
原生日常读取及不重新捕获该混合来源的操作没有被整体禁用，不能据此声称所有后续移动可用。
静态调用链中，拒绝发生于 plan → compose 的候选 State 克隆内，先于 publish_move；
真实混合来源再次移动的 source／target／磁盘不变回归尚未运行，不能称已有运行证明。
foreign identity remap 遇非空 path_audit 在改名或写入前拒绝；空 audit 的原能力保持。

## 格式与资源

旧 graph 缺少 path_audit，以及旧 CopiedLifecycle 缺少完整 path_audit 证据，均拒绝读取准入；
没有 serde default、静默补空、收据改写或自动迁移。拒绝不改输入原字节。
沿用既有 audit 字节／事件上限，并限制端点推导的累计步骤。
ledger 每次遍历共用一个原 source proof 冲突索引，不为每个 restore 重扫整个 State。

## 验证状态

实现附带 native／copied／restored 来源、二次导出、cold restore 后 copy、篡改、冲突、
不完整 batch、旧格式原字节保留以及增量／完整投影一致性的定向回归。
本地未编译、运行测试或格式化；是否通过由同一提交的 GitHub 定向 CI 工件判定。
多级备份、完整跨来源 foreign 导入和完整操作历史不属于本次声明。
