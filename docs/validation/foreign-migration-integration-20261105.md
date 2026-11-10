# 外来离线迁入包集成核验（2026-11-05）

本轮把独立 Rust 协议实验的迁入包实现接入 `storage-engine` 的
`foreign_import::migration`。模块、角色计划、恢复及预算见
[组件说明](../components/engine/src/foreign_migration.md)。没有外部推送、真实数据盘
操作或旧格式兼容；所有写入验证只使用临时目录。

## 本轮补强

- `copy_source` 必须使用计划原始物理目录，核对绝对路径、device 和 inode；相同
  逻辑摘要的新副本不能替代已审阅选择。复制后的独立校验仍允许新位置、新 inode。
- 持久意图额外绑定原目标父目录路径。原父目录移入源盘后，不能只凭同 device/inode
  在新位置恢复写入；恢复原名称后可继续同一操作。
- 不可变 marker 的读回比较与同步使用同一打开对象，不以重开名称后的字节
  验证另一个先前句柄。
- 补充物理选择替换、父目录移位与多次中断残留隔离测试，既有五个真实子进程退出
  窗口和独立异地校验继续保留。

## 实际验证状态

本轮相关 Rust 文件的固定工具链 rustfmt 与 `git diff --check` 已通过。
使用 15:57 UTC 统一重建的 Linux 引擎 libtest，运行
`TMPDIR=/workspace/shared target/native/debug/deps/storage_engine-4fb75adcb35977be
foreign_import:: --test-threads=2`，33/33 通过，用时 114.72 秒；包括 21 个迁入包
测试和 12 个只读预览回归。五个子进程退出窗口、异地复制后移除原来源及父目录的
独立核验，以及本轮三个新增拒绝／恢复场景均通过。
随后使用 16:16 UTC 重建、包含后续目录 checkout 修正的相同 libtest，再跑同一
过滤器，33/33 再次通过，用时 103.24 秒；不是新增 33 个不同测试。
最后在 16:45 UTC 共享重建、包含最终目录 checkout 修正的引擎上复跑，33/33
通过，用时 83.00 秒。

同一最终迁入包代码随后通过 `cargo clippy --locked --offline -p storage-engine
--lib --tests -- -D warnings`（exit 0，1 分 53 秒）；此次命令没有使用统一 Rust
诊断包装器，后续其他模块变更的合并验收不能由这份局部结果替代。
相邻普通目录转换测试补了真实观察边界刷新，冷恢复／停止后再开独立转换的
回归已通过。另发现并修正了生产路径中的健康样本过期窗口：私有准备值从来源
扫描开始持有准确已准入 Engine，穿过登记后仍由当前会话、准确目标和核心围栏
核验导入，导入前也保留服务停止检查。新请求的 15 秒健康门禁不变。
该最后一版扫描／登记跨阶段修正随后通过统一重建的 17/17 项普通转换测试
（31.07 秒），包括扫描后样本过期、新派发仍拒绝、队列撤权／停止、登记后撤权／
成员围栏，以及全部冷恢复、停止和重试场景。相同源码指纹也通过 server
all-targets strict Clippy；最终全 server 与 HTTP 集成结果另行记录。

## 共用固定样本修正

目录 Git 引入必填 `directory_git_repositories` 后，旧 canonical publication
样本在解析时被当前格式正确拒绝。本轮仅修正测试样本及 snapshot oracle 字段表，
未给生产解码增加默认值。使用独立 JSON 编码与域前缀 BLAKE3 工具，先核对所有
原始固定字节和 hash，再显式加入空 map 并重算 snapshot、history manifest 与
catalog 引用。原有 7 个 commit、4 个 version 和其他数据页字节全部保留。
增量 oracle 增至 36 张 map，并以非空嵌套目录 Git 值覆盖新增字段。

使用 16:45 UTC 共享重建的引擎 libtest，一次执行
`history_package::incremental_tests::`、
`history_package::history_changes_oracle_tests::`、
`journal::snapshot_incremental_oracle_tests::` 和
`canonical_value_encoding_preserves_publication_bytes_and_hashes`，34/34 通过
（156.15 秒）。保留完整对象原始字节、根、hash、增量重用及未知分支变更必须拒绝
的断言，没有只检查语义相等或跳过旧失败。

17:04 UTC 最终 engine all-targets strict Clippy 通过（51.13 秒），使用
`.github/scripts/check-rust-diagnostics.py` 包装器，包含本轮固定样本／oracle
修改及其后独立子目录 merge 修正，源码快照核对不变。日志为
`target/directory-git-resume/final-engine-clippy.log`。

## 未完成边界

此回执只证明离线迁入材料已完整落盘，不是活动磁盘接纳、角色转换或目录提交。
管理 `/execute` 仍明确拒绝。迁入包仍保留原盘现有全池证明，未裁剪为每盘只含自身
权威；逐盘发布／恢复协议、活动命名空间重建、系统 SQLite 新投影和完整磁盘向导
接线仍未实现。缺正文版本保留历史并明确暂不可用，不变成清除或迁移墓碑。

物理断电、挂载切换、所有 write/fsync 故障注入、性能压力与完整 workspace 验收
不在本轮局部测试覆盖内。
