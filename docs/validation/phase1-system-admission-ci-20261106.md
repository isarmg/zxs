# 一期系统准入与提案证明复用的独立 CI 接线

69 项接线基线为 `f5a6da65f7c4b04c0c1f29f0b62563b78ecc3237`，其父 `b0eb510ed66cd652b2722ef9806f874ea803f4a5` 与公开 `f6761067a07a71065540c5ca006eca32ade40aa8` 同 tree `95602b49196384172558a90cc328b38b29117565`。当前追加接线必须与提案证明复用候选 `e2085871a0050aa00ea4d502fa981233879ff502` 的准确生产源码和私有测试组合，不能独立用于未含该测试的主树；缺源文件在 Cargo 前失败，缺准确测试或被标为 ignored 在任何测试运行前失败。本接线不包含该生产修复，不改 Cargo 清单、根锁文件或工作流拓扑，不提高已有 job 的超时预算。

## 已确认的覆盖缺口

[旧候选的 workspace job](https://github.com/isarmg/zxs/actions/runs/42183296315/job/127778999237) 在根 Engine 组尚未产生完成摘要时取消。两项系统准入测试在该组中明确 ignored，随后 11 个 I/O 检查及三个尾部显式 ignored 调用尚未到达。原独立 core 的 67 项业务回归不能替代这两项真实内核边界检查，也不能说明完整 workspace 已通过。

后续同一准确 f676 源码的 [main job](https://github.com/isarmg/zxs/actions/runs/42188245775/job/127794674773) 已完成根 Cargo，包括 Engine `1991 passed / 43 ignored`、Server `1234 passed / 14 ignored`、集成目标和 doctests，零失败；此前未完成的 locator 用例最终通过。取消点转为第一个 `check-history-object-io.py` 的 package-only Engine 重编，未进入真实 I/O，后续十个 I/O 脚本及三个显式 ignored 调用仍未到达。运行约达 120 分钟，日志没有确认具体服务端取消触发；不能把旧候选的未完成状态继续描述成此次根 Cargo 的结果。

## 改动与资源边界

[core 驱动](../../.github/scripts/check-phase-one-core.py)在原来的 Engine 集成目标 `cargo test --no-run` 调用中增加 `--lib`。整个 gate 仍只有三次 Cargo 构建调用，使用同一 target 目录、根锁文件、`--locked --offline`，不新增单独 job 或另起一份 Engine 依赖构建。新增一个 Engine libtest 目标本身仍需编译；没有把这项新增成本写成零成本或已实测加速。

当前 Cargo JSON 必须给出唯一 `storage_engine` libtest，manifest、source、test profile、`fault-injection` 特性、可执行路径和 SHA-256 均受核对。构建日志、源头、根锁摘要、测试 helper 摘要和产物摘要进入现有证据。该图含正式 dev 特性，不称为无 hook 的生产构建。

运行任何测试前，驱动同时枚举完整 Engine 清单及 ignored 清单，核对两项挂载测试准确名字都存在且仍为 ignored，并要求新的提案测试存在且不是 ignored；不执行整组 Engine 单测。原 67 项原样保留，随后分别执行：

- `disk::admission_tests::immutable_existing_bind_leaf_is_rejected_before_read_or_sync`：真实 bind leaf 必须在两个原子写入口被拒绝，保留源字节。
- `disk::admission_tests::cold_directory_admission_keeps_held_identity_after_mount_replacement`：在私有 mount namespace 中更换挂载后，保留 held fd 的原身份与正文，拒绝新名字对应的身份。

两项沿用原测试断言。外层普通用户只运行准确 `--ignored --exact` 用例，清除可伪造子分支的环境 marker；只有现有 helper 的固定子链使用 sudo，Cargo 和整个 gate 不提权。原 workspace 全部命令、11 个 I/O 门禁及三个显式 ignored 调用仍保留。

新增普通测试的精确名字是 `engine::membership_transition::readmission::core::tests::verified_readmission_proposal_preserves_all_decisions_and_rejects_tampering`，来源为 `test/engine/unit/engine/readmission_core_tests.rs`。它使用同一次 Cargo 构建给出的正式 Engine libtest，以 `--exact --nocapture --test-threads=1` 执行，保留 300 秒单次预算；不带 `--ignored` 或 `--include-ignored`。该源文件摘要记录到 summary。原 69 项完全保留，新增一个普通用例，总计 70 项、10 个当前产物，仍为三次 Cargo 构建调用。组合源码必须产生自己的当前产物，不能拿默认 Engine rlib 小驱动、旧 f5a 或旧 e208 二进制替代。

## 完成与超时证明

挂载父 libtest 使用 terse 格式，子 libtest 固定 pretty、单线程格式。专门的解析器只接受一次准确子测试完成、一次父测试完成及各一份 `1 passed, 0 failed, 0 ignored, 0 measured` 摘要；两层 filtered 数必须与同一当前 Engine 完整清单一致。挂载 summary 只增加两项唯一用例，不把父子运行算四项。提案测试使用原普通单摘要解析器，严格要求一份 `1 passed, 0 failed, 0 ignored, 0 measured`；解析器不放宽。零匹配、缺少完成、重复摘要、ignored 或失败均不能完成 gate，失败时保留此前 69 项证据，不写成功总数。

sudo 清空环境后先进入 root 侧的 coreutils `timeout --signal=TERM --kill-after=5s 240s`，再进入私有 mount namespace 和准确 libtest。这样 root 子链自身有截止，而不依赖普通父进程能否向 root 发信号。驱动仍保留 300 秒外层等待；超时、sudo、namespace、mount 或缺失工具全部失败，不降级或跳过。外层提前中断时，root 子链仍可能运行到自己的 245 秒截止，不能称为即时清理已验证。真实超时清理仍须托管 CI 验证。

现有 `if: always()` 继续上传命令、stdout/stderr、当前 Cargo JSON 和 summary，保留失败、超时或未完成状态；不上传编译 target 或测试盘目录。

## 实际验证范围

本轮只验证 CI 编排，使用独立可执行夹具模拟 Cargo/libtest，覆盖当前产物选择、完整与 ignored 库存、父子完成、环境 marker 清除、非零与超时退出、缺少提案源文件、缺失／相似／重复／ignored 的新名字、零测试与重复摘要及旧 69 项保留。正式记录另列准确回归数量、退出码和源摘要；不能把模拟夹具通过当作私有 Rust 对抗用例已经通过。相对 f5a 的三个 Cargo 命令、十个产物约束、原 69 项名字、挂载 helper、workflow、workspace 命令及根锁保持不变。

父提交 f5a 已以固定 Rust 1.99.0 的无依赖小 libtest 实测挂载 parser 的父 terse、子 pretty 和 ignored 清单输出形状，临时二进制当时已删除。本轮不重编或重跑该探针，挂载 parser 与 helper 均未改动；旧探针不能验证新增提案测试、系统调用、提权、测试断言或超时清理。

没有在本机重编 Engine、运行 sudo 或 mount，也没有触发托管 CI。完整 static 未在该稀疏工作树执行，必须由组合候选补齐；上述回归不能称作两项真实内核测试通过。

上述 70 项基线的记录不包含下节新增 foreign 测试。此接线只让必要回归不被 workspace 长测阻挡，本身不修复 exit/readmission 的重复验证成本，不代替完整 workspace、其余 I/O 门禁、Chrome、Samba 或物理设备断电验收。

## foreign HTTP 准入接线扩展

当前扩展保留上述 70 项准确名字和全部断言，在既有 Server `cargo test --no-run` 中增加 `--test foreign_admission`，不新增 Cargo 调用、库特性图、job 或预算。必须组合独立测试交付 `ca4cfac385beb14dfa702d5f2f5cb08ed3425207` 的四条路径；CI 接线提交本身不含这些测试源码。Cargo 前核对 Server 清单中的准确目标、来源路径和 `required-features = ["core"]`，并记录测试主体与 fixture 的摘要，缺任一源码立即失败。

当前 Cargo JSON 必须产生唯一 `foreign_admission` test artifact，manifest 为 `src/server/Cargo.toml`，source 为 `test/server/integration/foreign_admission.rs`，具备 `default/core` 特性并通过既有可执行路径和 SHA-256 检查。全目标清单必须准确等于以下四项，不接受额外、缺失、重复或相似名字：

- `foreign_preview_requires_cookie_csrf_and_same_origin`
- `foreign_preview_rechecks_session_after_streamed_request_body`
- `foreign_preview_rejects_stale_sources_and_unreviewed_write_fields`
- `saved_and_revalidated_foreign_review_never_grants_import_execution`

仍在任何测试运行前完成全部枚举，共 12 次 list；foreign 目标串行完整运行，要求唯一 `4 passed / 0 failed / 0 ignored / 0 measured / 0 filtered` 摘要。总计 74 项唯一用例、11 个当前产物、3 次 Cargo 调用和 33 次准确过滤执行。原挂载父子 parser、非 ignored 提案测试、workspace 命令、私有持久 TMPDIR 及全部截止预算不变。若最后提案测试失败，只保留此前 73 项，不写成功总数。

本次仅执行相关 Python 编排／拒绝回归、格式及准确源码组合检查；没有 Rust 编译、运行、完整 static 或正式 74 项 CI 结果。旧 foreign 的四项运行和 focused Clippy 只对应 `b0eb510ed66cd652b2722ef9806f874ea803f4a5` 的默认 runtime，不是当前组合候选证明。准确完整组合仍须通过官方 `tools/rust-sources/manage.py refresh` 和 `verify --metadata`，再取得新的正式 Cargo／CI 证据；不得手改来源清单或将稀疏树检查称为全仓验证。
