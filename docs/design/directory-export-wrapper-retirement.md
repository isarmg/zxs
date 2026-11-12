# 无调用的旧目录导出启动入口退役

## 范围与依据

只删除 `business/directory_git/export.rs` 末尾的两个 Engine 公共 Rust 方法：
`directory_file_export_request` 和 `export_directory_file_history`。
它们是旧 SMB 目录自有文件导出的捕获与启动包装器；当前 HTTP、SMB、本机桥、helper、
工具和测试均不调用它们。两轮静态核查覆盖第一方源码、测试、构建脚本、宏与架构消费者登记；
该模块不受特性开关控制，Engine 无生成这些调用的 build.rs。
这不是为未知仓外 Rust 调用者保留兼容入口的承诺。

删除不引入替代操作，不恢复已停的非容器文件或非容器目录功能。
当前目录转换 `directory_conversion::export` 完全不变。

## 明确保留

- `file.directory.export` 的 CopyKind 识别、原请求、永久收据与恢复分派。
- `require_scope`、`committed_departure`、`apply` 与 retained 投影。
  Current publication 的 `advance_history_copy` 仍会按原 kind 解码和恢复。
- batch 使用的 `require_direct_scope_transfer`，以及提交目录自有文件跨域移动的明确拒绝。
- 普通文件改名、独立历史复制、现有容器正文、inode、节点、分支、标签及用户数据。
- 全部既有测试断言与原定向组；不为删除入口而删除恢复实现或扩大选择预算。

## 验证计划与边界

独立组 `directory-export-wrapper-retirement` 静态选出 8 个构建身份：
Engine 无默认特性产品 check，server core、helper、perf-check、browser-smoke 产品 check，
Engine fault-injection 单元测试，以及 directory_git_index 与 file_history_copy 集成测试。
按架构登记覆盖全部 Engine 运行时消费者，不用带故障钩子的测试构建替代产品构建。
三个 Rust 测试选择分别为 1、3、2 项，每项原超时 900 秒；完整变更合计 8 个 Python 模块。

行为覆盖包括：现有目录索引／收据冷开；可允许的普通文件改名保留正文、inode、标签、
历史与重放；已提交目录跨域导出及覆盖替换被拒且不发布；普通独立历史复制的全图、
故障恢复和原源保留。共享 history-copy 回归不是旧 `file.directory.export` 格式专属冷恢复
fixture，不宣称后者已独立验证；该恢复代码本片逐字保留。

未本地编译、运行测试或 rustfmt。实际通过与未执行范围以准确 GitHub 提交的
targeted-tests 工件、编译库存和逐测试结果为准。

## 首轮 GitHub 结果与后继补验

`82b15da6722c7bd812e61c3fa44eb5a4c89969a4` 的 CI #1274：
6 条选定 Rust 测试、8 个模块中的 124 条 Python 测试全部通过；
Engine 无默认特性、helper 与 perf-check 三项产品构建通过。
server core 与 browser-smoke 因前序 HTTP 退役误删共享 `patch` import 而编译失败，
不能据此宣称全部消费者已经闭合。

后继 `837858ea765755c31c75eb623d6daf6b43529fc0` 恢复该导入，
不恢复旧目录 PATCH 路由。本次文档跟进选择同一完整 8 构建／8 Python 组，
在修复后的代码上重验全部消费者和原 6 条行为，不跳过失败构建或减少断言。
准确结果继续保留在 GitHub targeted-tests 工件，未执行本地构建或测试。
