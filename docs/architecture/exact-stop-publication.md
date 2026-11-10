# 精确 Stop 发布计划与重试

## 边界

普通 Exit 与最后元数据成员退出在写入 bootstrap ledger 前，先持久化原操作的精确 Stop 计划。
计划绑定旧 head、可选的未停盘地址移动 record、最终 stop record、登记身份、operation 与 intent 摘要。
它只限制恢复探测集合，不能单独授权成员、降低固定 Q、改写绑定或代替真实 I/O 尾部闭合。
原 ledger 记录、Config、候选和永久操作收据的格式与字节不变；没有旧格式迁移或未知尾记录接纳。

## 普通 Exit

`membership-exit-stops/<operation>/plan.json` 保存完整 PortableAuthority 与精确 ledger 写集合，
SYSTEM `head.json` 绑定操作、authority hash 和 plan hash。只有真实保留根上的独立 certificate、
完整控制归档、登记目录 identity 与写锁达到原目标组固定 Q 后，才能构造不可反序列化的发布许可。
首次准备仍先完成原 Exit 的全成员控制安装要求；重试不会把保存计划或一个根内多份复制算作多个成员。

计划和 head 的 file/directory 屏障完成后才发布 locator。每次尝试重新核对并同步相同计划；旧 head、
中间移动 head、最终 stop head 及各自最多一个尚未被 head 选择的精确 record 是仅有的可重试窗口。
多余文件、未知临时文件、记录缺口、变更的 actor/operation/identity/authority 或另一条链一律保留并拒绝。
成功后保存同一 plan head 的 `complete.json`，但永久计划和历史 stop 不删除。

record 或 head 写入结果不确定时，只有已完成真实来源验证和计划持久化的路径才会关闭目标普通准入。
关闭并不等于排空：原 binding 继续持有，必须等待实际 descriptor、lease、writer 尾部到达 Closed，
才可发布 D3。准备阶段的失败不会错误关闭另一个目标；未知结果不被当作回滚。

普通启动在第一条存储根 stat/open 前拒绝未完成计划。`resume_membership_activation` 从精确计划获得
排除当前及历史已停目标的探测集合，再从实际保留根认证当前固定多数与完整归档；获得真实 writer 后
重复来源验证和 Stop 屏障，建立进程级 I/O 排除，然后按主线原有逻辑完成 marker、日志与 D3。
缺少计划时不能收养 dangling record，缺少实际 Q 或 archive 时也不能依靠 SYSTEM 副本继续。

## 终态退出

`terminal-management/<operation>/bootstrap-stop.json` 绑定已选完整 SYSTEM terminal authority 与精确 Stop。
终态重试由已验证的 SYSTEM archive 授权，不再探测目标、data-only 根或旧 Config 地址。
它同样先持久化计划和原 head，再发布 Stop；不确定发布会关闭活跃目标准入，但仍要求独立的真实尾部证明。
新计划不替代终态完成记录，也不修改主线对损坏 completion 或 selector 的具体错误来源。

## 与重新接纳协同

当 Disabled 磁盘已经显式重新接纳，Stop 必须关闭权威链证明的当前 admission epoch。
精确计划、真实来源许可和最终关闭证明都绑定这个 epoch；历史 Stop 不能关闭重新接纳后的新准入。
冷恢复先认证所有 Stop 的历史 epoch 和保留根的 SYSTEM probe 证书，再进行任何 storage stat/open。
probe 证书只给出只读探测许可，实际 certificate、完整 archive、原固定多数和物理身份仍独立核验。

## 成本和验证

每次停止新增一份受 MAX_METADATA 约束的 SYSTEM 计划，普通 Exit 另有选择 head 和完成标记。
locator 单记录维持原 128 KiB 边界，使用原有原子写和 file/directory fsync；未减少同步或裁剪历史。
只在停止、恢复、启动检查中读取计划，普通内容热路径不增加扫描。

回归源码覆盖四个真实 ledger 屏障的进程内和冷重试、地址移动的两段发布、旧 stop 保留、目标零内容写入、
实际保留多数及 archive 缺失、错误计划与未知临时文件、普通启动零探测和 busy I/O 尾部。
本批仅做静态核对、rustfmt 与差异检查，没有运行构建、测试、CI 或真实掉电验收。
