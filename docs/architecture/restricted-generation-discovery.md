# 受限代际发现：只读证据切片

## 当前边界

本片消费已经持久加载、校验并选择的 `RestrictedProbes`，验证当前实现支持的 **pure-add 代际**。它不是注销执行，也没有接入 `Engine::open`、健康探测或后台服务。没有 generation marker 的 Genesis 也不返回旧 Config，其受限认证需另外接线。退出代际仍由现有 `AuthorityNode::intent` 明确拒绝；没有为了测试合成退出权威或修改旧 Candidate、seal、archive 格式。

新增内部入口：

- `BootstrapStore::discover_generation(&ValidatedConfig)`：先 reload 完整系统侧 ledger/head，再纯选择允许根。无 head、损坏、较新记录不一致或未知配置映射均在第一条存储根 stat 前返回带原始来源的错误。打开系统状态目录失败必须由上层保留为 management-only，不能创建 locator 或回退旧 Config。
- `RestrictedProbes::discover_generation(&ValidatedConfig)`：要求现有不透明限制类型；没有 `Option`、布尔开关、`Default`、无约束转换或失败降级分支。再次纯核对传入投影与每个 probe 的 pool/disk/slot/binding generation/address，避免先对一个配置 select、再用另一个配置打开。
- 结果 `RestrictedGenerationEvidence` 仅包含实际 authority hash、完整 target 的固定 N/Q、通过成员及拒绝成员的具体来源。没有 Config、State、持有根、服务准入转换或写入能力。

旧 `discover(config)` 显式转给 `discover_unrestricted_legacy`。这里的 legacy 只是**当前未迁移调用路径**，不是旧格式兼容：Engine 两处调用、pure-add publication 和显式 activation resume 仍维持现有行为。完整生产接线必须在第一条目录 stat/open 之前改用限制入口，并继续完成停止意图、最新发布和资源准入核验；不能先调用旧 discover 再加 gate。

## 根与证明约束

1. 允许列表仅由 `ReadProbe` 构造一次，先完成全部纯检查，再执行存储 I/O。不会调用会预先枚举内容的 `disk_unavailable`。第一条命名根操作是该 probe 的 `symlink_metadata`，随后安全打开并与 locator 的 held identity 比较；匹配以前不读 seal、marker、journal、archive，也不枚举存储根内容。
2. fence/authority 中的其它根只能作为已存证明的内容，不能加入队列。受限扫描不跟随 fence 提示；marker 只用真实 portable chain 的验证与祖先关系选择候选。任一可见 marker 损坏或冲突均失败，不被另一个好 marker 掩盖。
3. 后续读取仅使用本次已打开并验证身份的 held roots。target authority 的路径不用于重新打开根。每个计数成员验证准确 binding、marker hash、Genesis membership anchor、portable joint enrollment、独立本地 certificate/真实 joint singleton 和完整控制 archive，再验证实际 seal 或精确前代/pending seal。前代 seal 只说明可读的准备阶段证据，不表示 activation 已发布。
4. `PortableJoint::verify` 保留原固定 old/target 独立证书门槛；另外要求达到完整认证 target 的固定 metadata Q。局部 archive、certificate、seal 缺失的成员不计数，不能为了补齐而探测被排除根。少数拒绝成员的具体错误随成功诊断保留；不足 Q 返回 `MetadataQuorumUnavailable`。这不等于“当前在线 old roots 达到 oldQ”，也不等于最新发布恢复完成。
5. 新 `JointStore::verify_certificate_read_only` 复用原纯证书验证器，只用 RDONLY 的 Joint namespace scanner 检查完整单帧及 receipt。现有 `verify_certificate` 会 synchronize，`PendingJournal::open_joint_at` 会取得可写文件，二者均不能用于这个入口。缺失、截断、torn tail 不创建、不修复、不同步；没有增加 fsync 或写屏障。

候选绑定检查当前为 O(P×C)，P 为允许根数、C 为投影根数；每个允许根至多一次命名 stat 和一次命名 open，其余均为 held-FD 相对读取。完整 archive 语义验证按实际被计数候选逐根执行，不以一个好副本代替另一副本；因此读放大与这些 archive 的总字节数成正比。没有新增持久写或 fsync。

root 仍指 ZXS 登记目录，不是设备。同一文件系统上不同目录可使用。已知身份重绑失败；不能把字符串检查或事后 inode 比较说成对所有外部路径别名、所有系统状态一致回滚的证明。

## 仍未完成

- 生产启动前系统目录定位、全部首次 I/O 的端到端限制接线。
- pending stop intent 与当前权威、最新 publication 的衔接。即使本片返回 Q 份纯添加代际证据，也不能据此解除 stop、更新 locator、重开服务或宣称选中了最新可写状态。
- 删除的 registry/namespace mask、退出 authority activation、持久生命周期终态。删除只踢出登记及目标独占当前列表，不创建或要求目标恢复文件/备份，也不擦原物理文件；固定多数、共享内容和必要控制历史仍须保全。
- OS 全进程 syscall 审计、真实断电以及生产离线/零 I/O 验收。

## 源码测试与验证状态

`test/engine/unit/engine/restricted_generation_discovery_tests.rs` 通过真实临时目录和原证书/archive 生成流程准备夹具。计数器在实际首个 `symlink_metadata` 和安全根 open **之前**记录命名根边界，不是在 DiskResources 接纳后计数；它不是完整内核 syscall tracer。

源测试覆盖：目标仍在线、旧投影仍命名已停止目标、真实 fence 和 marker 含其它根、固定 N=4/Q=3 而探测数不足、允许根 archive 丢失不得借被排除根补足、joint torn tail 不修复、坏 marker 不被好副本掩盖、held identity 替换先于内容读取失败、缺失/损坏 locator 和系统目录丢失不触及旧根、probe 被用于另一投影在 stat 前拒绝、允许根全部缺席无 Config fallback。成功路径对所有夹具根文件 bytes 前后比较，固定配置不变。

本机按任务要求只写源码，并使用固定 1.99.0 rustfmt 整理修改文件；**未运行构建、测试或重验证**。上述是新增源码断言，不是通过记录；须由父任务合批 GitHub 验证。

## activation resume 同批回归边界

部分 marker 故障可能只在第一个 data-only 根创建新代际目录；其它 metadata 根尚无该父目录。resume 的 fence precheck 现在逐级检查父目录，仅在真实 seal 仍属精确前代/本次 pending，且本代 marker 未存在时跳过尚未创建的 namespace。已 active seal 或本代 marker 却缺 namespace 明确拒绝。存在 namespace 时仍在 promotion 前扫描真实 fence，不吞其它 NotFound。

缺少 journal **leaf** 则维持原证据语义：marker 和 active seal 均可能先于首条 journal，不能一律拒绝；一个缺副本可由真实剩余日志证明恢复。所有当前 journal 缺失但仍有当前 publication seal 时，`reject_unjournaled_publications` 在建立 journal writer 前拒绝。另一成员仍有真实后继 fence 时，resume 在 promotion 前拒绝。新增两个负例保持这两个边界。

当前顺序仍会在 publication 矛盾检查前执行 promotion，对派生 archive/marker/seal 可能发生重新安装；它不会因此创建缺失 journal leaf。将纯矛盾检查前置应另行处理，不伪称本片解决了所有失败前零写。所有 fence 和对应独立证据被外部一致擦除，或整套当前日志与 publication 证据一并消失，不能由本片凭空检测。
