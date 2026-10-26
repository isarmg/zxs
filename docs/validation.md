# ZXS 验证体系

详细目录与来源约束见[测试结构](architecture/test-layout.md)。

测试实现统一位于仓库根 `test/`，按原组件和测试类型组织；通过原 Cargo 包的测试入口访问业务实现，不建立重复业务规则，也不为了外置单元测试扩大生产 API。

| 位置 | 范围 |
| --- | --- |
| `test/contract/unit/encoding/` | 严格 JSON、JCS、二进制校验清单及公开固定向量 |
| `test/naming/unit/` | 固定 Unicode、跨平台名称和路径预算 |
| `test/auth/unit/` | 两端共用的密码、令牌和 CSRF 原语及准入规则 |
| `test/client/unit/` | 真实本机 HTTP 下的流式校验下载、无覆盖发布和错误语义 |
| `test/platform/unit/` | 目录句柄、链接拒绝、原子落盘、SQLite 和凭据基础 |
| `test/engine/unit/` | 随模块维护真实文件、三成员日志、断点恢复、授权、独立历史包及容量拒绝回归 |
| `test/server/integration/` | 实际存储核心和 HTTP 请求的贯通验证 |
| `test/server/unit/smb_rpc/` | 本机套接字、FD 转交、句柄、读租约及保存边界 |
| `test/smb/unit/` | 受限帧协议和 SCM_RIGHTS 实测 |
| `test/smb-vfs/unit/asynchronous/` | 真实 SDK 事件库下的异步完成和取消生命期 |
| `test/interface/integration/` | CSR 状态与生命周期 |
| `tools/contract-check/` | schema、OpenAPI、固定样例和补充语义检查 |
| `test/evidence/` | 保留探针的源代码与固定输入，使用限制见[探针操作](validation/evidence.md) |
| `../test/tools/contract-check/gitignore.rs` | 临时 Git 仓库中的目录忽略策略回归，保护源码和固定样例，排除运行生成物 |

正文摘要的解析边界、固定字节 oracle 与保留的完整性检查见
[正文摘要类型边界](validation/typed-content-hash.md)。

## 执行与结果

执行入口见[CI](ci.md)、[浏览器冒烟](tools/browser-smoke.md)及[发行验收清单](validation/release-checklist.md)。结果须绑定实际提交、构建产物、输入和环境，失败、取消、跳过与未执行不能记为通过。

日志、截图、请求记录及结果 JSON 保存到 CI 产物或独立验收归档，不写成仓库中的历史报告。固定测试输入与测试源码一起维护；校验所需的不可变来源材料保存在 compliance，不因清理文档而删除。

核心故障注入仅用于可信测试进程，不提供远程故障开关。真实 Btrfs、设备故障、断电及 SMB 客户端矩阵必须在经授权的一次性环境另行验收；临时目录模拟盘、编译和浏览器冒烟不能替代这些结果。完整产品验收要求见[第 22 章](22-acceptance.md)。
