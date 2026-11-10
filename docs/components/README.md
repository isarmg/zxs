# 第一方组件说明

[文档目录](../README.md) · [单仓库结构](../monorepo.md) · [统一许可与来源](../../compliance/README.md)

以下组件属于同一根 Cargo 工作区，独立目录表示职责及编译边界。

| 组件 | 说明 |
| --- | --- |
| [认证](auth.md) | 可移植认证算法与令牌验证 |
| [命名](naming.md) | 命名规则、固定映射与合入的 Unicode 规范化 |
| [平台](platform.md) | Linux 文件系统、数据库与凭据基础设施 |
| [引擎](engine.md) | 存储、版本、分支、备份与恢复 |
| [服务端](server.md) | HTTP API、会话与本机桥接 |
| [日常管理](daily-management-coverage.md) | 当前管理页面、端点及操作边界 |
| [客户端](client.md) | 运维客户端的 HTTP 与完整性校验 |
| [辅助工具](helper.md) | 离线核验、恢复与历史包；磁盘管理逐项迁入，明确当前未迁边界 |
| [Web 产品界面](web.md) | 浏览器用户操作与 WASM 构建 |
| [浏览器基础层](interface.md) | CSR 原语、共享组件、主题、生命周期 |
| [SMB 桥接](smb.md) | Samba 协议桥接、VFS 与 SDK 边界 |

业务契约的唯一机器可读入口见 [contracts](../contracts/README.md)；Rust 类型位于 `src/contract/`。过程宏由 `src/macros/` 编译，原始许可与来源见 [集中通知](../../compliance/NOTICE.md)。基础实现的逐项来源、许可和当前路径见 [Rust 来源清单](../THIRD_PARTY_RUST_SOURCES.md)。

基础组件的能力与依赖边界见[基础能力目录](../architecture/foundation-groups.md)；具体参考包括 [Futures](futures.md)、[密码学核心](cryptographic-core.md)、[数据序列化](data-serialization.md)、[HTTP 传输](http-transport.md)和 [TOML](toml.md)。
