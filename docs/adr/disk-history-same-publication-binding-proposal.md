# DISK 变化史：先固定完整 SQLite，后由原生回执补足当前发布身份

**待确认；不是实现、验收或冻结。** 本文只细化已批准
[三范围方案](three-scope-production-authority.md)中的 DISK-history 来源绑定。
不涉及 FILE、认证、单盘救援、公共协议、投票者或新版本。若确认，新空池从第一天
使用同一结构；没有旧包兼容、双读或迁移。

## 为什么需要决定

当前 DISK-history SQL 把本次 Snapshot 摘要写入 header，把本次 Candidate 和
Snapshot 摘要写入每个本地变化行，`records_hash` 再承诺这些值。若本次 Snapshot
同时承诺这份 SQLite 的原字节，就是自身哈希环。只绑定此前 SQLite 再加逻辑增量，
不能独自证明后来安装的完整原始页；SQLite 完整、再编码出相同行都不够。

## 建议采用的唯一结构

保留现有 SQLite 两表及其严格实际 schema：`identity` 一行（格式、规范 header、
规范逻辑记录摘要），`changes` 按连续 ordinal 存储完整规范变化。仅改变两种 JSON
记录的来源字段；镜像仍是三范围表中已有的 `sqlite{schema,image_hash,image_bytes}`，
不增加物料类型或第二个文件格式。

| 记录 | 全部必需字段 |
| --- | --- |
| 规范 header | `identity,through,publications`；identity 为准确登记。through 是 `genesis{}` 或 `publication{sequence,operation_id,request_digest,predecessor}`。后者逐项匹配本次原请求和原决定的前驱（首笔为固定 genesis 锚摘要，后续为原 previous_snapshot）。 |
| 规范变化行 | `sequence,operation_id,source,change`。change 完整保留原来验证的 before/after 与本地稳定顺序。source 为 `prior{candidate_hash,snapshot_hash}` 或 `this_publication{}`。不保存伪摘要或双重身份。 |

序号保持原规范十进制；`publications` 沿原有上限及类型。genesis 没有变化，
覆盖数为 0；不是伪造一笔发布。新变化行只能用 `this_publication{}`，且其序号和
operation 必须与 header 精确相同。所有较早行必须是 `prior`，两项摘要由原先独立
验证的发布前缀供给，不由盘、路径或 SQL 自报。准备下一份不可变镜像时，只在
**新镜像**中将前一份当前行绑定为真实 prior；不原地重写前一份。没有本盘变化的
发布推进 header，不能伪造一条变化。既有 `records_hash` 域继续承诺完整规范
header 和全部变化行，不含本次 Candidate／Snapshot 的未来哈希。

## 唯一决定和事后核验

1. 由既有 owner 同时捕获已认证的完整前缀、此次真实原请求、准确前驱和完整本地
   before/after。正常 SQLite 编码一次，先同步、从目标 held 根读回整份真实文件，
   核验字节长度／摘要、完整 schema、规范行、身份、闭包和预算。原 Snapshot
   像其他 scope 一样直接承诺**这份完整镜像现在已知的准确字节**。
2. 原生 Publication／seal 成立后，验证器沿其准确 Candidate → Snapshot →
   scope → 本登记 → DISK-history 引用，再从本盘读原字节。只有全部物理及逻辑核验
   通过后，`this_publication` 才能在只读业务视图中解析为该原 Candidate 和
   Snapshot 摘要。较早行仍取其已认证原始来源。已有对外读视图／HTTP／SMB 不改。
3. 后验结果在内存中同时携带 `registration,sequence,operation_id,candidate_hash,
   snapshot_hash,logical_root,image_hash,image_bytes`。它证明本盘**完整最后物理
   字节**属于这次真实决定。无需新增可选择结果的 sidecar、日志票、提交标记或
   SQLite 发布点。失去图、文件或投票证据则保旧报不可用，绝不把本地最新序号升格。
   已决定后只能凭原 operation 和原决定补齐其已承诺的确切字节；不能撤销或重发。

不用 SQLite 页内补丁：那会引入地址区段、占位符、跨页处理、摘要填充和更多破坏
边界。这里在决定前镜像已完整，决定后没有任何物理字节要修改。也不依赖不同 SQLite
库版本把相同行再编码为同一文件。

## 合并后验收

新池空史、首笔／多笔及无本盘变化、原请求重试；覆盖跨页多行、有更早 prior 的
变化，及移植、错序、假 prior、伪 current、错误前驱和逻辑相同但物理不同的 SQLite。
核验只用独立原决定和所选盘；丢盘或恢复失败不能降 N/Q、改变业务历史或新建身份。
六路代码完成并合并后再运行。
