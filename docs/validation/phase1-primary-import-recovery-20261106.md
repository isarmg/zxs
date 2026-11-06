# 一期：单个一级盘初始化与普通文件导入恢复回归

本补丁将 `fdb96463890358464fb0f2fd8c2acd87185b0491` 的两项测试移植到统一一期 `2d983a9f19dc7c2ff2a4a623ada05feac37c6176`。只添加测试与说明，不修改产品实现。采用临时目录和既有故障注入特性，不访问真实用户磁盘。

测试文件为 `test/server/unit/setup_recovery/primary_import_recovery_tests.rs`，通过已有 activation 测试模块接线；测试源码保持与 fdb964 完全一致。无需合入旧 `7829`、`b823`、`d6` 开发链。

## 预期覆盖

第一个测试先通过真实 `activate` 初始化单个一级盘，在安装回调中断，再由新建 `SetupRecovery` 对象恢复同一配置、seal 和磁盘身份，固定 `N=Q=1`，保留未选择目录。

随后由真实普通来源清点和导入流程暂存 UTF-8 文本、二进制、零字节文件和空目录。在首次得到完整原 `CommitRequest` 的进度边界暂停，然后对同一原生提交分别注入 `after_accept_1`、`after_finalize`、`after_publication_1`，避免误打盘根创建或未完成的暂存操作。

各故障均须得到有 `InjectedFailure` 源的 retryable `InDoubt`，只读且不发布内存状态。冷启动后恢复原操作一次：序号只加一，原工作区变为 `Committed/F+1`，原请求重试不再增长。核对精确五项路径、类型、每份正文以及来源和已接受暂存文件的 inode、模式与字节。

三个边界分别验证导入恢复后的两种激活收尾：已有 COMPLETE 时不重做安装；之后独立丢失 COMPLETE 时恢复已有池和原回执。六种组合不证明“同时缺激活回执且仍有未发布导入候选”可自动产生新决议。

第二个测试在 `after_finalize` 后删除两份测试夹具的原请求记录，保留全部正文。全局发布仍可恢复，正文仍可读，但工作区保持 `Open/F`，完成谓词为 false，调用者提供原请求重试仍须拒绝；不补写请求、不创建替代工作区、不改变原证据。

## 统一基线验证状态

本轮已完成精确 2d 基线的私有 Git index 应用检查、测试源码身份比较及 rustfmt 检查；没有重新构建或运行这两项统一基线测试。静态兼容结论不能代替运行结果。

2d 的现有默认生产 Engine 没有 `fault-injection`，不能直接运行上述故障测试。Server 已有 dev-dependency 为 Engine 开启该特性；准确候选应用本补丁后，应在隔离测试产物目录运行：

```sh
cargo test --locked --offline -p storage-server --lib setup_recovery::activation::tests::primary_import_recovery:: -- --test-threads=1
```

这两项测试预计执行六个正例组合和一个原请求缺失负例。不要把组合场景数记成独立测试数。还应运行原有七项初始化保护；私有测试需要真实 Engine 和 Server 测试目标，不能复制实现或伪造 fixture 来替代。

## 历史证据与边界

fdb964 源码上的历史运行是 27 项通过：一期定向 9 项、普通转换 17 项、有界 ENOSPC 驱动 1 项；它们不属于 2d 组合验证。ENOSPC runner 及配套文件不在 2d 树，本补丁不为复用这段历史引入旧开发链。

既有准确 2d 的公共 Backend 驱动另有发现、准入和浏览 10 项通过，可复用其经过源码／产物 SHA256 核验的依赖与真实服务夹具；它未覆盖上述普通导入故障恢复。旧 fdb 测试二进制不能证明新链接实现。

本补丁不声称完整 CI、物理掉电、真实满盘、HTTP／浏览器或 SMB 验收通过。外来盘迁入和多级备份不在范围内。
