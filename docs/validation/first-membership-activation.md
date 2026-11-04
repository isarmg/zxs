# 首次纯添加成员激活：实验批次验证

本批次实现 [ADR 0013 的首个纯添加闭环](../adr/0013-generation-bound-membership.md#first-pure-add-activation)，基于已提交的 fence、冻结归档、pending 准入和 joint 证书阶段。它是独立验证分支上的实验实现，不是已发布能力，不得仅凭这份清单合入或开启 Add/Delete HTTP、Web 或真实部署。

不要求旧程序兼容，也不运行旧二进制探测。保留当前用户数据、原始 Candidate 字节与哈希、永久结果和历史证明是本批次的数据正确性要求。

## 当前证据状态

截至 2026-11-04，最终批次尚未在 GitHub Actions 执行，不能报告最终源码通过。

- 中间源码构建的单元测试 binary：16 个激活测试中 15 通过、1 失败。失败的 workspace 镜像用例原先覆盖同一文件，未真正改变选择上下文；当前源码已改成新增文件并在提交后切换文件分支，尚待重编执行。
- 同一中间 binary 的受影响过滤已通过：membership 95（另有 1 个原有忽略 fixture）、普通 recovery 28、proof decoding 7、journal scanner 10、workspace storage 10、history package 33、scrub 58。七个先前回归均已定向通过，分别涉及盘身份错误、共享证明故障通知和 pending 故障恢复。这些过滤存在交集，不能累加为独立用例总数。
- 更早的一次广泛回归被主动中断，未完成，不作为本批次通过证据。最终格式、生产构建、Clippy 和所有定向结果以提交绑定的 Actions 输出为准。
- 最后源码新增或修订的 degraded 永久结果覆盖、继承的 `AcknowledgedUnavailable`、真实 workspace 旧镜像恢复、旧/新历史包独立 import、成员 epoch 结果断言均未执行。不得以中间 binary 的结果替代。
- 本地重型验证已停止；后续按源码批次交给 GitHub Actions，不逐处修复重跑完整 CI。

## GitHub 验证入口

[专用 workflow](../../.github/workflows/membership-validation.yml) 对 `verify/membership-*` 分支 push 运行，也提供手动入口。新 workflow 未进入默认分支时，以专用分支 push 作为第一次执行入口，不依赖手动入口已经可见。

现有 CI 只排除该验证前缀的 push，main、其他分支、tag、pull request 和手动完整 CI 保留原规则。验证分支不自动创建 PR，也不自动合并。

一次运行完成以下范围：

1. 读取仓库固定工具链，初始化独立 Cargo home；只使用仓库本地依赖、根锁文件与 `--locked --offline`。
2. 检查 workflow、架构和 `storage-engine` 包格式；不执行全仓格式改写。
3. [定向脚本](../../.github/scripts/check-membership-activation.py) 只构建一次 `storage-engine --lib --features fault-injection` 单元测试 binary，随后复用它串行运行激活、成员协议、恢复、降级恢复、证明解码、日志扫描、workspace、历史包、scrub、盘身份和读取故障通知组。不链接无关集成测试，不运行被忽略的旧二进制 fixture。空过滤组视为错误；某组失败仍收集其余组的结果。
4. 单独检查不启用故障注入的生产库，再检查受影响库 Clippy；构建诊断保留并拒绝 Rust 警告。最后检查文档契约。
5. 只上传文本日志、Cargo JSON 诊断及绑定 HEAD 与测试 binary 摘要的结果 JSON，不上传 binary、缓存或真实磁盘材料。Actions 的提交 SHA 和所有步骤结果共同构成证据，不能仅以某个测试组成功宣称整个批次通过。

当前激活过滤预期有 17 个测试；计数不是永久配额，增删测试须同时审阅覆盖范围。正常 CI、发布验收以及后续整合批次需要的完整验证不由这项定向 workflow 替代。

## 必须通过的实际边界

- 旧池写入 → fence/冻结归档 → 旧/新独立 quorum 的 joint 决定 → 新代际首个永久结果 → 新业务写入 → 重启与同操作幂等；原普通日志及旧结果保持。成员 epoch 不再固定误报为 `1`。
- 各持久屏障中断后显式恢复：marker、seal、首个 Prepare/Accept/Finalize/Publication 和系统配置投影前。只读启动不推广 pending seal；部分已发布结果不能退回冻结 State。
- 固定新集合 N/Q；一旧一新形成 2/3 合法多数时，即使原 2/2 多数不可用仍能恢复已发布数据，缺员时禁止普通新写。单个坏锚隔离且坏盘所有文件字节不变，两个真实冲突权威仍拒绝。
- 系统目录丢失、成员目录移位、已知 ID 的路径投影不被归档旧路径抢占；未登记的新增空目录不能阻断原池 fenced 只读恢复，也不能被启动过程改写。
- `EmptyGenesis` 首次添加后发布第一条业务；准确 sequence + hash 的旧候选解析；严格新 Candidate 字段与 authority；拓扑溢出、操作冲突、后续 fence 明确阻断。
- 仅保留 data-only 盘时，独立 check/export 旧、新正文；缺少旧 inclusion 页或新 authority 依赖必须失败，不能向系统或其他成员回退。新墓碑对旧正文生效。旧、新历史包独立 check/import 并恢复对应正文。
- 激活前、后各自已提交 workspace 的系统镜像陈旧，且当前文件选择已进一步改变时，仍恢复其当次准确快照上下文。
- 冻结前的降级已发布结果保留最终 recovery 覆盖；历史不可用副本摘要按实际 publication 序列播种，后续恢复副本由既有规则撤销旧摘要。scrub 验证冻结归档时遵守现有读取、取消、限速与对象预算，缺件/损坏不以新日志正常为由漏检。

## 明确不包含

重复成员迁移仍拒绝，后继不能冒称 Genesis。下线、排空、删除、退休成员返回、一般多代链和 UI/API 仍未实现。纯添加不移动原正文、不更改原路由，也不声称新增正文副本。

便携证书聚合容器以现有 64 MiB 元数据上限预检；没有完成大规模实际成员激活或吞吐验收。临时目录故障测试不证明物理掉电、硬件缓存、跨主机 fencing、恶意回滚或克隆设备安全。复杂度与同步预算见 ADR；本批次没有短暂停或性能提升承诺。
