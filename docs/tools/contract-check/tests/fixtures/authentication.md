# 唯一管理员认证字段样例

`test/tools/contract-check/fixtures/authentication/` 的六份 JSON 是合成的非秘密字段样例，分别覆盖登录、当前会话、注销与改密的请求或响应。令牌不是运行服务签发的凭据，样例也不证明密码校验或网络端点已经通过验收。

`test/tools/contract-check/unit/main_tests.rs` 按四个现有 OpenAPI 路由读取对应 schema，验证必填字段、封闭对象、到期时间十进制编码，以及角色、禁用、仓库准入和账户集合等旧字段被拒绝。固定向量由日常 `contract-check` 同时校验。

登录候选用户名的规范化和密码的 UTF-8 字节规则由共享认证实现验证；字符串 schema 不冒充密码验证器。会话撤销、Origin/CSRF、并发请求、持久化失败和 bootstrap/恢复仍须由服务端实际测试证明。
