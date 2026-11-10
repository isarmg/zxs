# SMB Read 借用解码验证

日期：2026-11-03。范围是本机 SMB RPC 客户端接收，不改变 wire、能力协商、服务端授权、
Foundation 源码或真实部署。

## 边界与所有权

- `FrameStream::receive_payload` 仅提取原有传输体；长度、整帧期限、部分读及 SCM_RIGHTS
  处理不变。原 `receive` 仍通过 `decode_strict` 准入普通响应。
- `StrictJson` 只有完整严格验证构造器，私有不可变切片借用将证明绑定到原输入。
  与 `strict_json` 共用一个编译期物化模式 visitor；重复键在读取其重复值前拒绝，
  仍拒绝浮点、非法 UTF-8／surrogate、尾随内容及原深度上限之外的输入。
- Read 的信封 visitor 将字段按原输入顺序插入实际 `json_codec::Map`，仅将已有 `result`
  字段替换为 Null 并借用原字节。原 `Response` 继续决定信封准入；默认 Map 和
  `preserve_order` 特性分别保留原来的键排序与插入顺序。
- 顺序仍是完整 JSON、信封、FD 合约、Read DTO、请求 extent。成功 DTO 直接解码字节；
  DTO 拒绝时使用原 Value 路径生成诊断，保留错误类型、code、来源和首错字段。
- 所有工作仍在原共享连接锁内，RawValue 不逃出本次 exchange。错误时收到的 OwnedFd
  释放、所有 clone 关闭；完整业务错误保留 operation_id、retryable、errno 和连接复用。

未删除任何安全或完整性检查，没有增加协议兼容分支。额外 JSON 扫描仍为 O(帧字节)，
对象重复键检测仍保留当前对象的解码键。正常 Read 不再保留 N 个字节 Value；
其余信封字段及错误慢路的内存上限仍来自原帧上限。锁次数、系统调用与同步次数不变。

## 独立 codec 测量

一次性探针在相同工具链、依赖及 `opt-level=3` 下比较实际
`storage_contract::decode_strict::<Response>` 基线和本次生产借用解码 helper；
同时将基线与改动前冻结 visitor 交叉核对。未使用 release LTO，也未测 socket、Samba、
磁盘、目标机器吞吐或 RSS。输入帧预先构造，成功结果在每次测量内释放。

分配探针使用独立计数 allocator，记录 alloc/realloc 请求总字节与账面同时存活峰值；
不包含预先存在的输入帧，不代表 allocator 实际保留页或 realloc 内部暂存峰值。
时延探针完全不注册该 allocator，5 轮交替新旧次序；每轮小输入 1,000 次、64 KiB 输入
40 次，下表为每轮单次均值的中位数。两种探针不能混用时延证据。

| Read 字节数 | 分配请求字节：旧 → 新 | 峰值存活请求字节：旧 → 新 | 分配／扩容调用：旧 → 新 | 无计数器时延：旧 → 新 |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 2,107 → 2,063 | 1,475 → 1,281 | 9 → 11 | 673 → 931 ns |
| 1 | 2,396 → 2,071 | 1,635 → 1,281 | 13 → 12 | 953 → 941 ns |
| 255 | 43,034 → 2,567 | 17,827 → 1,281 | 25 → 17 | 14,691 → 9,495 ns |
| 256 | 43,067 → 2,567 | 17,859 → 1,281 | 25 → 17 | 15,462 → 9,415 ns |
| 65,536 | 11,601,723 → 133,127 | 4,195,779 → 65,536 | 42 → 25 | 4,644,926 → 2,032,288 ns |

满块的账面峰值减少约 98.4%，单次 codec 时延中位数减少约 56.2%；空块增加约 258 ns，
1 字节的差别小于本次测量噪声，不宣称所有长度都更快。这里省去的是 N 元素中间数组，
不是 N 次独立堆分配。早期 BTreeMap 中转版本的结果不作为上述最终实现证据。

## 语义回归与实际验证

独立探针为 436 个输入各比较有／无 FD、0／64 KiB extent 的 1,744 个 codec 结果，
与实际严格解码 oracle 无差异。FD 布尔组合只验证 codec 阶段优先级，不证明资源生命周期；
真实 socket 回归另行执行。

[Read 差分测试](../../test/smb/unit/client/response.rs)覆盖 448 个 payload：
顶层形状、状态与字段顺序、缺省／未知字段、重复解码键、错误重复值、UTF-8／surrogate、
数字边界、尾随数据、深度和全部 u8 值；比较实际 I/O kind、具体来源类型、contract code、
JSON 行列／分类和完整错误链。多错误信封会检测 `preserve_order` 下的首错漂移。

[真实客户端测试](../../test/smb/unit/client/read.rs)覆盖实际 SCM_RIGHTS 的错误优先级与关闭、
原客户端及两个 clone 的 read/call 失效、业务错误后 clone 再读、排队旧成功帧不被复用、
短成功回复后失败只保留已验证前缀；原有范围、EOF、传输、Open 和写入回归继续保留。

复跑入口（均使用仓库原有依赖，不下载 Rust 包）：

```sh
cargo test --locked --offline -p smb-protocol -p storage-contract
cargo test --locked --offline -p smb-protocol -p storage-contract --features json-codec/float_roundtrip
cargo test --locked --offline -p smb-protocol -p storage-contract --features json-codec/preserve_order
cargo test --locked --offline -p smb-protocol -p storage-contract --features json-codec/preserve_order,json-codec/float_roundtrip
cargo clippy --locked --offline -p smb-protocol -p storage-contract --all-targets -- -D warnings
```

CI 的 `smb-protocol` 分片同样执行默认、float_roundtrip、preserve_order 及二者组合，
由脚本调用回归核对，不把特性矩阵留为未执行的文档建议。

本机最终实现的四种特性组合均为 SMB 58 passed、contract 142 passed／3 ignored；
双包 Clippy（all-targets，`-D warnings`）与双包 rustfmt 通过。完整 static 汇总、
SMB 四组合 CI 入口及 45 项 CI 脚本调用回归也实际通过。

本记录不将单元、socket、静态检查或纯 codec 测量视为真实 SMB 客户端吞吐、发行构建、
全仓最终提交 CI 或设备验收。`storage-contract` 原有三个隔离／手动测试仍为 ignored，
不将其计为通过。

## 2026-11-10：现有元数据载荷贯穿到 VFS

客户端将同一借用 result 准入用于 Open、Stat、AuthorizeRead 和 DirectoryPage，删除单独的 Read／Open 结果分派模型。服务端保留原 DTO 至 transport 的 ResponsePayload 序列化，VFS 接收已准入 DTO 后仅转换宿主 ABI；不再把元数据先转 Value 再逐项读取／反序列化。字段和值的协议保持，对象字段输出顺序可变化，不用于持久摘要。普通操作回执仍为原 Value，没有更改操作身份或发布流程。

坏 DTO 和无进展目录页在同一连接锁内拒绝并关闭共享会话，避免其他 clone 继续消费错误响应；目录页保留原 EINVAL，原生 errno 与完整远端业务错误不改。StrictJson 全帧与重复键、信封、FD 配对、DTO 和 Read extent 的先后保持；既有拒绝诊断的 Value 慢路继续保留。服务端每次发包的帧预算、一次锁／一次 RPC 与目录游标更新时机不变。

根测试新增三种元数据的业务拒绝后复用、坏 DTO／多余 FD 后所有 clone 失效、无进展目录页及四种 DTO 的差分错误链；保留原 448 个 Read 样本。这里只做静态和固定格式检查，未本地编译或执行本轮 Rust／Samba。上文历史测量与通过数量不属于本次实现验证，新的准确提交 CI 待执行。
