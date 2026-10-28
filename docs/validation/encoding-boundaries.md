# 规范 JSON 与 SMB 打开响应的重复工作边界

基线为 `005e584bb27dec4d2d118a278850997e6af72670`。本次仅移除已经证明可复用的
对象键排序和打开结果重复解码；不改变公开哈希字节、RPC 帧或持久化协议。

## 规范 JSON：按真实迭代次序选择捷径

本地 [`json-codec::Map`](../../src/foundation/data-storage/json-codec/src/map.rs)
默认使用 `BTreeMap`，保留的 `preserve_order` 特性改用 `IndexMap`。不能以当前
业务字段多为 ASCII，或当前生产依赖没有启用该特性，推断所有对象都已规范排序。
`storage-contract` 的依赖特性还可能被其它消费者统一启用。

[`encoding.rs`](../../src/contract/src/encoding.rs) 对每个对象进行一次线性扫描，
仅在全部键为 ASCII 且实际迭代已经严格递增时直接遍历键值对。ASCII 字节次序与
UTF-16 代码单元次序相同；空对象满足条件，单个非 ASCII 键仍走原路径。含 Unicode
或乱序的对象继续收集键并使用原 UTF-16 比较器排序，不按 UTF-8／标量次序替换。
字符串转义、安全整数范围、浮点拒绝和域前缀仍由原边界维护。

静态成本：命中对象省去临时键 `Vec`、排序和逐键 Map 查询，改为扫描加直接遍历；
未命中对象保留原排序复杂度，并增加至多一次键扫描。未测量端到端耗时、分配总数
或吞吐，不把此前[整数写出测量](canonical-number-encoding.md)当作本次性能结果。

[`object_ordering.rs`](../../test/contract/unit/encoding/object_ordering.rs) 对照冻结的
完整 emitter，并核对公开泛型／Value 编码以及三个域的字节和哈希。覆盖：

- 空／单成员对象、控制字符／引号／反斜杠／DEL 键、嵌套对象。
- 不同长度、轮换和逆序插入，既检查真实 Map 顺序，也比较最终字节。
- ASCII 与 BMP 私用区／补充平面混合；固定向量明确要求补充平面排在 `U+E000` 前。
- 不安全整数和浮点继续拒绝；重复键（含转义等价键）、非法转义和孤立代理项仍拒绝。

现有 `contracts` CI／release shard 的
[`check.sh`](../../.github/scripts/check.sh) 分别运行同一 oracle，且不改变生产清单：

| 显式附加 json-codec 特性 | Map 后端 | 用途 |
| --- | --- | --- |
| 无 | BTreeMap | storage-contract 默认消费者 |
| `raw_value,float_roundtrip` | BTreeMap | 本地 HTTP/SQLite 与 schema 消费者的编码特性并集 |
| `preserve_order` | IndexMap | 保留的插入序后端，不假设默认顺序 |
| `preserve_order,raw_value,float_roundtrip` | IndexMap | 插入序后端与实际生产编码特性并集；std 和 alloc 仍由相应依赖启用 |

`storage-contract` 自身依赖 std，不把这些运行当作 json-codec 的 no-std 验收。
本次不扩展为 all-features；未影响 Map 后端的其它可选特性保留原独立消费者验收。
[`test_check_suites.py`](../../test/ci/test_check_suites.py) 核对命令接线，不代替 Rust
执行结果。完整 `encoding::tests`、固定发布向量和数值 oracle 仍由原工作区测试执行。

## SMB：锁内准入一次，转移类型和 FD 所有权

[`Client`](../../src/smb/src/client.rs) 的私有 `AdmittedResult` 区分普通 JSON 值和
`OpenResult + OwnedFd`。共享连接锁覆盖发送、受限帧接收、严格 JSON、FD 对应关系和
打开元数据准入；成功打开结果在此消费 Value 解码一次，随后直接交给 `open()`。
不把原始打开结果泄漏给调用方重解析，也不增加通用序列化框架或兼容分支。

`call()` 现在只返回无 FD 的普通 Value；Open 请求在发送前以 `InvalidInput` 拒绝，
调用方必须使用 `open()`。VFS 中四处消费普通结果的解构已同步调整，其它非打开
调用继续忽略普通返回值。传输 Response、OpenResult wire 字段和实际 VFS 打开入口不变。

FD 与请求／响应不匹配仍优先拒绝；元数据非法、严格 JSON 失败和传输失败在解锁前
移除共享连接，收到的 OwnedFd 自动释放，所有 clone 随后返回 `NotConnected`。
完整远端业务错误不关闭连接。Namespace 传输不确定结果仍保存原 operation ID、
原始错误和不可自动重试标记。帧长度、重复字段、SCM_RIGHTS/CLOEXEC、期限和串行化
策略不变；不引入 writev、多路复用或协议降级。

新增 [`admission.rs`](../../test/smb/unit/client/admission.rs) 覆盖打开全部元数据与
FD 的独立生命周期、错误入口不发送、不合法原始 JSON 后全 clone 断开与 FD 释放、
打开业务错误后的连接复用，以及 Namespace 超时保留原操作身份。原测试继续覆盖
缺失／意外 FD、未知字段／错误数值、坏帧残留响应及成功打开后继续使用连接。

## 验证状态

本地仅进行源码修改、固定 Rust 1.99.0 逐文件 rustfmt 和 Git 差异检查；
Cargo、构建、测试与来源治理生成均在 GitHub 执行。没有真实部署、权限／凭据变更、
数据迁移或运行池清理。

精确输入提交 `ebc86d170566c4bd14c9162ad4c7791dd0da6010` 的
[正常产品验证](https://github.com/isarmg/zxs/actions/runs/40635350996) 已实际执行：

- contracts job `122641918742` 的四种显式特性配置均使用 `--locked --offline`，
  每轮四项对象排序回归通过；`preserve_order` 轮实际编译当前 indexmap 实现。
- smb-protocol job `122641918828` 的 39 项测试通过，其中 client 20 项包含五项新增
  admission 回归；真实 Samba SDK job `122641918672` 的 27 项测试与 release 消费者通过。
- workspace job `122641918812` 再次执行对象排序回归，并通过全部 31 项 telemetry
  测试。以上是指定输入和 job 的证据，不替代后续组合提交的完整验证。

此前两次官方[生成](https://github.com/isarmg/zxs/actions/runs/40634223995)与
[诊断](https://github.com/isarmg/zxs/actions/runs/40634796626)均通过：Cargo 1.99.0
的 `metadata --offline --features json-codec/preserve_order` 解析图包含
`foldhash`、`indexmap`，来源、架构、文档 refresh 和 static 检查通过；生成差异为空，
包版本集合不变。根锁文件字节未变，其 json-codec 条目仍只有原四项依赖，不能把
metadata 图中的可选边说成已写入锁文件。未手改锁文件或 catalog；上述真实
`--locked` 特性矩阵是该输入能够构建并执行测试的证据。
