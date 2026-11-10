# 容器名称投影：按既有写集筛选文件

本切片仅收窄 `container_storage::changed_files` 的候选键，不改变容器格式、名称初始化、实际副本、授权或发布顺序。基线为 R37 owner 提交 `f97739b6959d5cb91ea8d959be195b9d43b5bf4c`。

## 消费者与保证

普通发布在复制正文前调用 `validate_new_projection_paths`，安装正文后、ACCEPT 前调用 `prepare_names`；恢复分别通过普通准备或已验证 Publication 的 `repair_historical_names` 重建投影。这些消费者仍各自取得完整的受影响文件集，没有新增跨阶段缓存或跳读能力。

私有 `container_storage/changes.rs` 直接消费现有 `Tracked::changes_since`：

- `Keys` 只界定可能写入的键，仍执行原有值比较、存在性判断和前后归属解析。同值写入、插入后删除不制造投影。
- 名字、路径、备份配置、备注、选择按文件键比较；路径节点／元数据节点／merge event 保留原新增或删除语义。备注仍按原先的路径节点优先级解析，版本备注及 parents 仍解析前后版本归属。
- 版本候选来自 versions、placements、content_states 三份写集，不能只看新增 FileVersion。固定路由、绑定代数、主槽、份数和内容状态保留原比较规则；topology_generation 单独变化仍不新增名称投影。
- 分支仍只比较 pending 引用、节点、名称、状态和 history；被删除的 pending 引用仍从前态收集，普通 generation 递增不产生投影。
- 写集精确性依赖现有非内部可变记录契约及同一容器 lineage。冷解码、foreign replacement、未知 bulk mutable 操作、键数／字节上限产生 `All`，明确扫描前后权威记录，不猜测遗漏键。版本三字段中任意 `All` 都执行原全 versions 路径。

该分类器不新增外部准入，也不替代 `RawState → AdmittedState`、历史包验证、当前权限检查或提交线性化检查。原磁盘身份、长度／哈希、成员规则、独立完整副本、原子名称替换和 fsync 均未变；已发布修补允许缺失副本的既有例外未变。

## 成本与明确保留项

精确写集时分类成本为 O(K log(N + K) + S)，K 为相关候选键数，S 为实际候选记录内 pending map、路径集合等的比较／输出成本。版本三字段用借用键的集合去重，不复制版本键字符串，每个候选版本只比较一次。空精确写集不遍历全图。`All` 仍按实际相关 map 的全量规模处理，不宣称冷恢复增量。

`capture_names` 仍在业务 closure 前后遍历主分支 entries；当前准入并不保证名字和位置字段完整，不能直接删除这两个扫描。`projection_locations` 仍为每个受影响文件遍历版本；容器 JSON 构建仍遍历其既有集合与图节点。名称分类仍被两个生产阶段独立调用。本切片不声称端到端 O(delta)，不修改 descriptor 图／旧依赖的 byte verification 或安装策略。

## 验证入口与状态

新增根 `test/engine/unit/container_storage/changes_tests.rs` 保留基线完整扫描作为独立 oracle，覆盖每个输入 map 的增删、同值、临时键回退，版本／placement／content 联合变化、前后文件归属、context 目标变化、pending 移除及分支非投影字段、冷解码／foreign／bulk／写集溢出。独立 helper 测试用禁止消费的全量迭代器验证 Keys 分支不遍历未触及记录；生产不添加计数器、差分断言或测试参数。

既有 `container_storage`／`projection_path_tests`、名称重放、完整正文 inode、坏目标拒绝、历史包与恢复回归仍应执行。此次提交是 source-only 候选；仅完成静态阅读、固定 1.99.0 工具链对三个改动 Rust 文件的 `rustfmt --edition 2024 --config skip_children=true` 及 `git diff --check`，未执行本地 Cargo、编译、单元／集成测试或性能测量。运行结果须以该候选在获准 GitHub 验证分支的实际证据补充。
