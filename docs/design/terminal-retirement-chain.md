# 逐笔终态登记后继与已认证旧 Stop 消费

[任务清单](../../TASKS.md) · [首笔基础](terminal-retirement-successor.md) · [辅助工具](../components/helper.md)

基于真实 `027d7252` 开发并对齐真实父 `89c69d65`。仅交付源码，未审查、编译、测试、浏览器验证或运行/查询 CI，未运行 helper 或操作任何真实系统/盘。此片扩展终态**管理登记移除**，不是 metadata readmission、活动命名空间或外来完整低级链正式接纳完成。

## 不可变前驱与准确下一笔

保留原 terminal authority、归档、bootstrap 根、固定 N/Q、全部历史定义、业务历史与先前永久结果。第一笔仍用原目录内的 `retirement-*` 文件。每笔完成后可选另一个仍在当前 registry 的准确绑定：来源为当前 registry commitment、State sequence/topology、主体及新 operation，不接受在前一笔之前取得的旧预览。

下一笔先在已完成目录不可变写入 `retirement-next.json`，包含**完整下一 intent、其摘要及准确前驱完成摘要**。该文件同步后才创建原 terminal 操作目录下 `retirement-operations/<operation>/`。新目录采用与首笔相同的 intent/selection/successor/Stop/completion 协议。没有可重写的“当前成员”清单，也不从某个孤立子目录选权威。

父链接已同步、子目录尚未创建或子 intent/selection 部分落地时，冷恢复只用父链接中的完整原意图补同一个目录/ID/正文；不借 helper JSON 猜目标，也不生成新 ID。每笔完成后才能越过其 next link。错误前驱摘要、源 registry/sequence/target 改变、复用永久 ID、未知条目或未被链选中的子目录失败并保留材料。祖先完成证据损坏/缺失不会被子链接替代。

最终登记可逐笔到零，仍是只具管理权威的空登记，不是可写零成员池或首次初始化许可。历史拓扑不会因移除而删除。metadata-resume 的早期门禁也拒绝 `retirement-operations` 目录证据，即使根选择文件损坏/丢失，也不先探测数据盘。

## 旧 Stop 不是新移除结果

未 Stop 绑定继续沿首片追加精确新 Stop，再取得真实进程 I/O 排除。若当前 epoch 已 Stop，仅当当前登记确是 Disabled，且原 final-voter PlannedOffline 或完整历史 portable authority 独立认证了该准确 pool/disk/slot/generation、原操作、原 intent digest 与当前 epoch，才可消费它。只有 locator hint 或 Disabled 字样不够。

该分支在当前帧写 `retirement-existing-stop.json`，绑定原已认证关闭生命周期与新 retirement/successor 摘要，重新同步原 locator 屏障，但**不新增重复 Stop、不清除旧 Stop、不重启或探测目标、不创建新的 PlannedOffline**。fresh 与 existing 两种 Stop 计划同时存在视为冲突。最终 completion 仍需当前 durable exact Stop、完整 successor、真实已关闭 I/O 域与准确永久结果，复用旧 Stop 不复用旧注销结果。

## 原结果、未决状态和 helper

每个已完成操作的主体、Plan、永久结果及进程排除保留于 owner 生命周期；冷打开从准确 SYSTEM 链逐笔重新验证并重建，不把缓存或 helper 文件当权威。较早 operation 可按其原 actor/ID/digest 查询，重复完成返回同一个原结果，不改变最新登记或 sequence。新未决选择不抹掉之前的成功：服务 `completed_retirement` 可与另一个 `pending_retirement` 同时展示，相同 ID 的冲突组合拒绝。

helper 复用既有三项 terminal-retirement 接口和原根计划，只有当前精确预览能开始下一笔；若已有未决，必须使用原根摘要。页面不再声称只支持第一笔，仍单独确认视图损失及**不能恢复 metadata/活动存储**。沿用手动 root、默认 `0.0.0.0`、无登录、Origin/CSRF、任务互斥和服务 UID/host lease，不修改网络或系统服务。

## 仍然关闭的边界

当前 registry successor 只表达管理登记，保留历史但没有可供 active namespace 消费的新 occurrence-mask/history 闭包。fresh/retained metadata resume 和 readmission 仍拒绝，不能从旧 Config 复活已移除盘。没有取消、擦除、格式化、卸载或失败自动回滚。损坏祖先证据不是可自动恢复的正常掉电窗口。

备份当前虽已有原成员首笔 Publication/seal 和逐路 CommitProof 源码，仍没有可安全激活的目标 Config/Current/serving locator/target writer 或源解围栏公开 API。因此外来二级及以上盘的完整低级链正式接纳仍拒绝，不借普通 Add、此管理链、材料或布尔意图返回成功。H03 未完成。

## 成本与未运行证据

新增下一笔只创建一个有界完整链接和一个私有操作目录，再沿原每文件/目录 fsync 屏障写入。内存只保留原始、当前前驱/当前 successor 的 registry 图；各历史帧只保留 Plan/结果/排除 guard，不为每次移除长期复制全量 State/registry。正常管理提交仍需编码当前 registry、克隆 State；冷重建需逐帧检查前驱/图/locator 并重建 State，成本随历史和登记增长，没有吞吐优化声明。未知目录检查只限 SYSTEM 私有链，不遍历数据盘。

新增测试**源码**涵盖两笔到空登记、下一笔中断与较早原结果、持久 next link 而子目录缺失、孤立子目录拒绝、历史 Disabled Stop 不变但新移除结果独立，以及 wire 的不同未决/完成 ID 共存。全部未执行；沿用的历史 PlannedOffline fixture 也不代表新产品动作获准或已经验证。
