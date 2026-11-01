# ZXS 认证基础

此 crate 是管理员用户名、Argon2id 密码、会话令牌、CSRF 和浏览器 Origin 校验算法的唯一实现，不依赖 HTTP 框架、Linux 或数据库。算法与参数保持现有契约；服务端与离线救援工具直接依赖 `authentication`，平台不提供认证兼容导出。

唯一管理员凭据状态、会话到期、请求限流和持久化由服务端管理；核心不持有用户 ACL，负责对象归属与存储事务约束。本 crate 不处理存储业务约束，也不提供账户集合或角色模型。

验证：`cargo test --locked --offline -p authentication`。

Origin 边界由 `verify_same_origin` 接收完整 Origin、Host、Sec-Fetch-Site 头值列表。
直连 HTTP 支持非回环主机；默认监听地址由 server 配置，不属于认证限制。
`ExternalOrigin` 在配置准入时将单一 HTTPS 来源缓存为浏览器序列化；请求 Origin
必须精确等于该值，或直连 Host 派生的 HTTP 来源，不归一化请求 Origin 后放行。
代理内侧 Host 不必等于外部域名，但提供的 Host 仍严格检查单值及 authority。
重复（即使值相同）、逗号合并及非法头值拒绝，Fetch Metadata 可省略。
无 Origin 的普通 Bearer/只读请求仍允许单个 Fetch 值；提供 Origin 时 Fetch 必须
为 `same-origin`。setup 的只读 bootstrap 状态保持更严格的既有 Fetch 要求，
由共享 `require_same_origin_fetch_metadata` 实现。Cookie 变更必须有 Origin 及单个
有效 CSRF；Bearer 不要求 CSRF，但不能绕过已提供 Origin 的校验。

原回环 HTTP 模式及独立 WebSocket Origin API 已移除，不保留兼容接口；仓内只有
测试消费者，已迁移到当前产品向量。公开库仓外调用方不可在本仓穷尽确认，须迁移
到当前 API；这不是声称旧接口无外部消费者。实际 peer 与可信代理来源选择仍归
server 负责，Origin 证明不代替会话认证或代理信任。URL 解析错误保留具体 source。
