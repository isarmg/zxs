# ZXS 文档

[项目入口](../README.md) · [工程规范](20-engineering.md)

文档只维护当前使用、设计、开发、验证和发行所需的说明。历史改进过程、评审报告、单次测试结果和发布日志不在此重复保存；提交历史、CI 产物及 GitHub Release 承担相应查询用途。维护文档以中文为主。

## 使用与运维

- [快速开始](getting-started.md)：构建和启动服务。
- [操作手册](25-operation-manual.md)、[配置](21-configuration.md)、[systemd 部署](operations/deploy/systemd.md)。
- [管理面](management/control-plane.md)、[初始化与恢复](management/setup-recovery.md)、[SMB 集成](components/smb.md)。
- [当前支持范围与限制](23-delivery-and-decisions.md)、[发行与安装](release.md)。

## 产品规则

| 主题 | 规范 |
| --- | --- |
| 总体 | [需求](01-requirements.md)、[概念与不变量](02-concepts-and-invariants.md)、[架构](03-architecture.md)、[平台与入口](04-platforms-and-access.md) |
| 存储模型 | [命名与路径](05-naming-and-paths.md)、[文件历史与分支](06-history-and-branches.md)、[数据模型](07-data-model.md)、[盘格式](08-full-storage-and-disk-format.md)、[备份拓扑](09-backup-topology.md) |
| 一致性与安全 | [事务恢复](10-transactions-and-recovery.md)、[写入](11-write-semantics.md)、[读取](12-reading-and-cache.md)、[保留删除](13-retention-and-deletion.md)、[权限](14-security-and-permissions.md) |
| 接口与维护 | [API 与错误](15-api-and-errors.md)、[客户端](16-client-interfaces.md)、[生命周期](17-lifecycle-and-maintenance.md)、[故障处理](18-failure-handling.md)、[容量与性能](19-capacity-and-performance.md) |

[专题设计](design/README.md)和[设计取舍](adr/README.md)解释当前规则；文件版本历史属于产品功能，不是项目改进记录。

## 开发与验证

- [编码与优化规范](../禁止防御性编程.md)、[项目指令入口](../AGENTS.md)。
- [单仓库结构](monorepo.md)、[工程规范](20-engineering.md)、[命名规范](NAMING.md)、[测试布局](architecture/test-layout.md)。
- [实现架构](architecture/README.md)、[组件](components/README.md)、[开发工具](tools/README.md)。
- [第三方依赖文档](dependencies/README.md)：按职能查阅当前用法、匹配版本的本地原文与实际 API。
- [机器契约](contracts/README.md)：schema、OpenAPI 和固定向量。
- [最终本地 RPC 成对验收与 SMB 组合语义](validation/final-local-rpc-acceptance.md)。
- [CI](ci.md)、[质量门禁](quality.md)、[验证方法](validation.md)、[产品验收标准](22-acceptance.md)、[发行检查清单](validation/release-checklist.md)。

## 来源与许可

[统一合规说明](../compliance/README.md)、[第三方通知](../compliance/NOTICE.md)、[Rust 来源清单](THIRD_PARTY_RUST_SOURCES.md)和[原始上游资料](upstream/README.md)服务于来源校验与分发，不作为项目进展记录。许可证、来源身份和校验所需的不可变原件不得按文档精简规则删除或改写。

## 维护规则

1. 一个主题保留一个权威入口；索引链接到它，不复制大段内容或运行结果。
2. 文档描述当前行为、命令、约束和限制。新增或修改功能时同步相关说明、契约与测试；不新增日期命名的改进报告。
3. 测试日志、截图、计时和构建收据保存在 CI 产物或独立验收归档，不混入规范。可复用检查清单与验证方法可以维护；没有证据的能力不得宣称已验收。
4. 产品不承诺旧版本接口或磁盘格式兼容，但用户数据、权限、完整性和恢复边界仍须保护。现有运行池的清理、重置或迁移必须单独授权。
5. “必须”“不得”表示强制要求，“应”表示默认要求，“可以”表示允许的能力。设计要求不等于已经实现或验收。
