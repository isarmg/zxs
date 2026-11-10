# 已验证 Range 缓冲与首次分支结果

本批以 `624facd4cadb5f0c9b731cb3f06be95d60731edb` 为集成基线，依次纳入
`143a49a5`、`4c9e536a`、`2930ea03` 的源码候选。本文说明边界与验收入口，
不以定向格式或静态检查代替编译和运行；最终验收须绑定中央验证的实际提交与 tree。

## 下载缓冲与失败来源

`transfer::VerifiedRange` 仍先读取完整触及块并核对 BLAKE3，只在验证通过后
暴露该同一私有缓冲的请求范围。尾部 `truncate` 后把 Vec 移入现有 Bytes，
再通过 `Buf::advance` 调整起点；不再用 `Vec::drain` 移动非对齐首块的剩余字节。
每个块仍有独立所有者，继续读取及销毁 reader 不会覆盖已经返回的块。

外部 Range、公开 Backend 清单形状及范围检查保持。副本顺序、完整块哈希、
成功副本置首及逐块重新验证保持，不以已成功读取的副本推断下一块完整性。
seek/read 失败仍允许其它副本完成验证；全失败保留外层 `InvalidData` 及原文本
`CHECKSUM_MISMATCH: no replica has a verified payload chunk`，私有具名错误携带
首个真实 I/O source。纯摘要不符不虚构 I/O source；成功 fallback 不再返回或记录
已处理的错误。已开始的二进制响应仍按原规则终止为流错误，不追加 JSON。

没有改动分块清单输出、块宽、上传／读取／后端许可隔离、read lease、期限或
取消语义。R26 `HttpBackendJob` 的初始授权／打开 owner 与许可转移保持；
已验证内容块后续 I/O 仍是原例外，不把 AuthStore 绑定到响应正文或通用 Permit。
原清单响应的小块复制仍保留，未以本次正文优化改变其资源策略。

## 首次分支创建与永久重放

`Engine::create_branch` 使用原 `plan_mutation_digest` 和字节不变的 change 闭包。
首次 `Prepared` 从同一未发布 `next` 取得刚构造的 Branch，执行原 `persist`
成功后才返回；不为返回值复制并解码自己刚生成的 `created_branch` JSON，
也不为返回 OperationResult 额外克隆一份完整 result。必要的 Branch clone、
receipt 编码、候选状态与源条目复制保持。

`Existing` 仍只解码永久 receipt 的冻结创建记录，不能用后来修改的 live branch
回退。缺失或损坏 receipt 保持 `StateInvalid`、原消息及具体 JSON source。
actor、canonical 摘要、operation owner、写入准入、代数、时钟及发布次序均沿原
planner／persist；持久化、索引或 InDoubt 错误不得返回已经捕获的本地 Branch。
内部 `expect` 只表达本次 change 同时构造记录和 branch_id 的不变量，不替代持久
输入边界。receipt 不增加 TTL、裁剪、格式兼容或迁移。

R25 三 prepare 的外层 Result 与当前身份优先于字段错误保持；R26 job helper、
认证强 owner、许可期限及明确例外保持。未恢复旧 BackendAction、generic call、
dispatcher 或分散 auth 捕获；未改公共 API、model、schema、通用 planner 或 persist。

## 验收范围

源码测试仍位于根 `test/` 并使用既有接线。下载新增覆盖同一分配地址、块独立性、
全部合法块宽和 Range 精确字节、实际 I/O source 选择及成功 fallback；分支新增
fresh／receipt 全字段与规范形状、损坏 receipt、错误优先级和五个真实 fault 点，
并强化后续写入／归档／重开后的原始记录重放。

源码组装阶段仅执行钉定工具链的六文件定向 rustfmt、diff whitespace、Python 架构
检查及 fixture、来源／manifest／锁文件收据和消费者索引静态核对。该阶段未执行
Cargo（包括 `metadata`）、编译、Rust 测试、Clippy、负载或性能测试，未建立 target，
未联网或推送。这只记录源码组装范围，不代表后续集成验证状态。
完整验收须绑定最终提交及 tree，执行新增及既有 engine／server、默认／no-core、HTTP、
撤权／取消／许可释放、fallback／传输限制和发布／恢复回归，并单列其它契约、来源与
发行验收。

结构上减少一次非对齐首块 memmove 及首次创建返回路径的 JSON 往返；没有新增
锁、I/O、fsync、依赖、配置、生产 trait 或 hook，也不声称实测吞吐、分配或峰值内存收益。
