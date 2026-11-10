# 构建、来源与验收工具

[文档目录](../README.md) · [统一工作区](../monorepo.md)

所有工具由根工作区维护，在仓库根用 `cargo ... -p <包名>` 选择。工具链、目标 SDK 与浏览器仍按各自平台准备。

| 工具 | 说明 |
| --- | --- |
| [Web 构建](web-build.md) | 正式 WASM、绑定、静态资源与构建输入收据 |
| [绑定生成](web-bindings.md) | 本地维护的 Web-only 绑定命令 |
| [Rust 来源管理](rust-sources.md) | 集中来源、原始身份与逐文件完整性 |
| [契约校验](contract-check.md) | schema、固定向量和活动文档链接 |
| [Unicode 维护](unicode-build.md) | 固定映射与 Unicode 表重现 |
| [浏览器验收](browser-smoke.md) | 实际浏览器、WASM 与真实服务端 |
| [性能检查](perf-check.md) | 存储回归和测量证据 |

发布版本与产物校验工具见[发布指南](../release.md)。测试的模拟结果、真实浏览器与实际 SMB 互操作、物理盘故障及正式发布应分别说明。
