# 一期目录仓库最小安全差异：c95a

本轮基于 `c95a9eff8abe72911dc2a4a7bb0191d3cbf64041`，只提取 f966 之外的三项安全修复。保留 f966 的工作树/index隔离、挂载相对路径、历史dirty状态、分支门槛与现有DTO；不公开新 `index_tree_id` 字段，不叠加 e274 的30文件整包，不加入兼容或迁移。

## 修复范围

- 正文清理把所有目录 Git 分支的 HEAD、自有 index 文件及明确固定的子提交作为保留根。父 index 的原始快照里，稳定独立子边界内的工作正文不作为父自有暂存根；当前历史祖先本身不额外固定正文。检查复用原 purge 准入，保持旧错误优先级，一次惰性扫描，不执行正文 I/O。
- 提交验证真正的自有暂存来源：当前登记的 unavailable_versions 与 ContentState::Available 均须满足，不把后续working完整误当成旧index完整；固定子提交仍是历史引用，可以落后于当前子HEAD并保留不可用历史正文。
- 同路径重建后，旧目录分支不能用新operation ID提交新目录。比较当前稳定目录ID；旧仓库已有HEAD时不能错误返回no_change，尚无HEAD时不能进入unborn断言。准确旧请求仍重放原永久回执。单纯删除未重建时，f966本已拒绝，不把它算作新增修复。

生产差异仅5个Engine文件，无额外同步阶段、锁、数据迁移或真实盘操作。保留根扫描按所选版本过滤，committed树根去重；自有文件按快照中的稳定子ID及祖先路径排除，成本随保留index/HEAD条目及路径深度增长。稳定身份检查另扫描当前命名空间；原状态预算、发布闭合和完整State恢复验证不变。

## 公有API回归与基线结果

[目录安全回归](../../test/engine/integration/directory_git_safety.rs) 为5项、单P01加SYSTEM、quorum和副本数均1；[暂存来源回归](../../test/engine/integration/directory_git_staged_source.rs) 为2项，使用临时目录完成真实D3退出流程。全部只读State、走公有API，无伪造State、私有hook或真实设备；只准备purge计划，不执行purge。两个文件均接入Cargo独立目标。

复用已证明生产源码等价于准确c95a的默认Engine运行库（无fault-injection），只链接两个小型driver。最终基线共运行7项：6个预期反例失败、1个保留历史的正向对照通过。

| 基线场景 | 实际结果 |
| --- | --- |
| 未提交旧仓库被同路径新目录替换 | 触发unborn repository断言panic |
| 已提交旧仓库被同路径新目录替换 | 错误返回成功的no_change回执 |
| T1仅由自有index固定 | 错误允许purge计划 |
| T1仅由Git HEAD固定 | 错误允许purge计划 |
| C0仅由父明确暂存的子引用固定 | 错误允许purge计划 |
| T1/P01暂存后保存T2/D04，退出P01 | 仍成功发布不可用自有T1的用户提交 |
| 父固定旧子C1，子推进C2，退出旧正文盘 | 成功保留固定历史并支持冷重开和准确重放 |

失败场景在首个热态断言即结束，不能声称这些基线失败场景的后续冷态或引用释放阶段已运行。测试源码为修复后运行保留了相应hot/cold、无副作用、明确restage和引用释放后的计划许可断言。

两个driver的最终链接用时0.967秒及1.078秒、峰值RSS不超过502780KiB；运行库未重编。c95a Engine库SHA256为 `eda119e45526d3f24a24198ede762368c60ac35e06bd562a4e3a4b992a95131b`，测试链接前后核验依赖及源码摘要。生产修复另通过 `rustc --emit=metadata -D warnings`：14.985秒、峰值1123912KiB，单CPU/2GiB地址空间上限，不生成运行库或大型单测二进制。

完整原始命令、日志、源码摘要与仅生产补丁保存在本任务独立运行目录 `runtime/directory-safety-review`，交接时另提供manifest及patch/mbox；早期4项基线记录保留，最终统计仅使用后续5项driver与2项D3 driver，避免重复计数。

第一方架构静态门禁通过：20个owner、31条声明边。Engine依赖、特性和package表与基线逐值相同；仅新增两个测试目标并更新对应manifest摘要。为了在稀疏副本执行门禁，只补出不可变基础库Cargo清单，未检出或编译基础库源码。

## 尚未验证

遵守“不重型构建”，未重编修复后的Engine rlib，因此7项回归尚未链接修复库运行；metadata通过不能替代这一步。整合者应在统一构建的新运行库上重链接这两个driver，验证全部7项通过，再按组合改动选择现有10项directory_git_index及HTTP等检查。未执行全仓测试、完整Engine单测、故障注入、真实Chrome、SMB网络或真实盘掉电验收。没有推送。
