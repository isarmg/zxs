# 磁盘登记的类型化拒绝入口

基于 R24 源码 `bdbcf3576741e911306c8735b766a1ed23057ad2`；本轮仅源码验证，
不能据此声称 R24 或本轮 Rust 测试已通过。

## 契约与边界

`POST /api/admin/disks` 没有线上登记实现，也没有被消费的请求字段。HTTP 保留既有
严格 JSON 格式检查（空正文接受、任意合法 JSON 接受、重复键拒绝）、1 MiB 上限、
30 秒正文时限及 backend 容量准入。没有引入空 DTO、字段约束、配置或兼容参数。
Web 当前只提供磁盘 GET 检查，客户端无登记调用；离线 rescue 登记不受影响。

Backend 专用 `register_disk(&Identity) -> Result<Infallible, ApiError>` 明确没有成功值，
不再传递 JSON action。HTTP 外层身份认证先于正文；AuthorizedBackend 当前身份检查
先于底层调用；Engine 在执行锁内重新准入后维持原 409 及完整错误消息：
`disk registration requires an explicit offline configuration and topology migration`。
ManagedBackend 保留服务停止／存储不可用先于 Engine 调用的错误顺序，不把原 503
变成 409。无 core 的测试后端仍明确返回构建不支持 501，不伪造登记成功。

请求取消不释放仍在执行的 backend 槽位；blocking closure 明确持有认证所有者，
避免 Rust 精确字段捕获提前释放认证 Arc。JoinError 在 HTTP 边界转换，panic 不泄露
内部消息。不改变磁盘、持久化、幂等收据、同步或物理复制预算。

## 已补源码覆盖

- 已登录固定 409/code/message/retryable；合法标量／对象／数组／空正文，无事件写入。
- 非法 JSON、重复键、超限、匿名先于正文、退出后的凭据撤销。
- Managed 存储未 ready 的原 503；无 core 测试后端的 501 和格式错误优先级。
- 全部 Backend 实现，授权透传结果和错误、真实 panicking 方法；容量槽位、取消后
  槽位保留、认证 Weak 在 worker 完成之前存活而完成后释放。
- 静态撤权清单增加 RegisterDisk：86 种入口、86 个 execute 分支、86 条四路矩阵
  （共 344 组合）和独立 86 条无凭据矩阵。强认证所有权取消清单为 13 条。

以上数字描述该独立作者分支。R25 接续合入三类 prepare 后，最后 dispatcher 与宏
均已无消费者并删除；89 条入口矩阵和 16 条强所有权路由见
[最终源码集成](final-typed-actions-integration.md)。body/path 边界保留。

## 独立源码分支的验证限制

定向 rustfmt、diff 检查、来源 refresh／verify（只执行 Cargo metadata）、消费者索引
和轻量架构检查由独立源码阶段执行；后续中央运行记录以交付报告为准。该阶段没有 Cargo build/test/Clippy、
运行时／socket／浏览器／SDK 或性能验证；没有新 target、push 或 main 修改。
