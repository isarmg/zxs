# 生命周期切换读取的有界成员复用

候选只改变内存读取成本，没有新持久字段、DTO 或合法编码。

`LifecycleSource::capture` 仍扫描所有收据并保留无关坏 ownership 的拒绝语义；同一趟扫描可附带构造
最多 65,536 个 typed file/operation/类别的成员索引。超过上限只放弃该索引，不截断目标成员、不返回
假末页，也不扩大事件上限。索引没有原收据、完整 D、主体表、路径、凭据或强 State 引用。

只有目标事件、当前 D 覆盖、全账本 path/copy/Graph 封套、引用／碰撞及最终排序全部成功后，才将
成员索引放入每个 Engine 的一个独立槽。它与原单文件 Event 槽共同使用实际 `Weak<State>` 分配身份；
换文件只命中成员，不能拿另一文件的事件。值相等的另一 State 不命中，失败／取消不保存不完整来源。

读取时临时升级 weak 引用，仅使用现有 Tracked lineage 核对 captured receipt/principal/graph/D
相关表没有被替换，再立刻释放；缓存不获得读权限，也不持有 State 或永久操作表。替换／修改捕获表
不能被原 Event 或成员槽掩盖。原逐次权限、history root、位置、sequence、游标、取消和页面预算不变。

切换文件后仍进行完整 path ledger 准入，包括无关坏封套；只少一次 ownership 分类扫描。
首次冷读仍要扫全部收据，cache 超限也回到原路径。没有宣称首屏 O(page size)、总账本已索引、
重复完整封套解码已消除，或大历史实际延迟已达标。

新增测试源码：

- `lifecycle_membership_index_counts_bounded_file_operation_memberships`：真实类别和多文件归属计数；
- `lifecycle_membership_cache_overflow_drops_optimization_not_target_members`：上限后保留全部目标成员；
- `lifecycle_switch_reuses_memberships_but_changed_capture_readmits_full_ledger`：真实 Engine 两文件，
  只复用同一成员 Arc、事件槽换目标，替换为坏收据的捕获重新拒绝且原 State 不改；
- `lifecycle_membership_cache_uses_exact_state_and_never_holds_its_authority`：等值不同 State 不复用，
  缓存不增加强 State 引用。

保留现有冷热／复制／删除／取消／无关坏收据／Event 预算回归。以上源码均未执行；本切片未运行
编译、测试、formatter 或 CI。合并后的准确 head 应定向验证，不把静态检查记为行为或性能通过。
