# 任务一端到端测量：分页命名空间根 + 分段事件/回执日志

本页数字是 Release 1234 交付方提供的 Rust 1.97 容器测量记录，尚未由项目固定 Rust 1.99 的独立对照验收复现。原始 JSONL 保留如下，不作为本轮修复后的性能结论。

表中的读取／写出字节取自 `/proc/self/io` 的 `rchar`／`wchar`，表示系统调用层逻辑字节量，不是块设备实际 I/O；设备计数是原始记录中另外列出的 `read_bytes`／`write_bytes`。

测量入口为 `test/engine/bench/task1_bench.rs`（显式 example，普通测试不运行，
`cargo run --locked --offline --release -p storage-engine --example task1_bench -- <phase> <files> <state-dir>`）。
基线为独立 worktree 上的旧平铺实现（main @ fc16eeb3，加入同一测量文件的
平铺 API 变体，未改其业务代码）；新实现为本分支。两边以 release 构建、
相同负载与档位逐阶段独立进程运行：`VmHWM` 因而反映单阶段峰值，
`rchar/wchar` 为该阶段进程内逻辑读写字节（`read_bytes/write_bytes`
为实际块设备字节，受页缓存影响仅作参考）。

负载与阶段（三档：500 / 1 500 / 3 000 个文件，97 个目录；档位上限受本
容器磁盘额度限制——基线实现的每版本发布证明随平铺 State 增长，保留池
约为 1.4 KiB×N² 量级，50 000 档在旧布局下无法放入本环境）：

1. `build`：单工作区暂存全部文件后一次发布（初始化成本）。
2. `edit`：重新打开后连续 20 次单文件保存（局部修改的复制/编码成本）。
3. `read`：1 000 次路径点查 + 一页目录列表（200 项）+ 一页事件（200 条）。
4. `open`：重新打开引擎（恢复成本与常驻内存，附 VmRSS）。

每档结束后另记 `pool_size`：该池在盘上的保留字节（`du -sb`）。

原始输出见本文末尾 JSONL；下表为关键比值摘要。

## 结果摘要

| 档位(files) | 指标 | 基线(平铺) | 新实现(分页) | 比值(基线/新) |
| --- | --- | --- | --- | --- |
| 500 | build 墙钟 (ms) | 10467 | 10928 | 0.96× |
| 500 | edit 20次读取字节 (MiB) | 173.4 | 56.2 | 3.09× |
| 500 | edit 20次写出字节 (MiB) | 17.0 | 17.6 | 0.97× |
| 500 | edit 峰值内存 (MiB) | 51 | 34 | 1.51× |
| 500 | read 1000点查+两页 读取字节 (KiB) | 0 | 144 | 0.00× |
| 500 | read 峰值内存 (MiB) | 84 | 42 | 1.99× |
| 500 | open 恢复墙钟 (ms) | 1606 | 6025 | 0.27× |
| 500 | open 读取字节 (MiB) | 387.7 | 459.8 | 0.84× |
| 500 | open 后常驻 VmRSS (MiB) | 60 | 38 | 1.56× |
| 500 | 保留池字节 (MiB) | 332.8 | 333.4 | 1.00× |
| 1500 | build 墙钟 (ms) | 58170 | 62730 | 0.93× |
| 1500 | edit 20次读取字节 (MiB) | 488.9 | 144.9 | 3.37× |
| 1500 | edit 20次写出字节 (MiB) | 48.0 | 48.7 | 0.99× |
| 1500 | edit 峰值内存 (MiB) | 124 | 74 | 1.66× |
| 1500 | read 1000点查+两页 读取字节 (KiB) | 0 | 376 | 0.00× |
| 1500 | read 峰值内存 (MiB) | 224 | 98 | 2.29× |
| 1500 | open 恢复墙钟 (ms) | 6296 | 13099 | 0.48× |
| 1500 | open 读取字节 (MiB) | 3048.4 | 3230.0 | 0.94× |
| 1500 | open 后常驻 VmRSS (MiB) | 150 | 86 | 1.74× |
| 1500 | 保留池字节 (MiB) | 2893.1 | 2893.7 | 1.00× |
| 3000 | build 墙钟 (ms) | 239497 | 220104 | 1.09× |
| 3000 | edit 20次读取字节 (MiB) | 973.9 | 289.8 | 3.36× |
| 3000 | edit 20次写出字节 (MiB) | 91.9 | 93.3 | 0.98× |
| 3000 | edit 峰值内存 (MiB) | 239 | 104 | 2.30× |
| 3000 | read 1000点查+两页 读取字节 (KiB) | 0 | 725 | 0.00× |
| 3000 | read 峰值内存 (MiB) | 419 | 129 | 3.24× |
| 3000 | open 恢复墙钟 (ms) | 178422 | 133343 | 1.34× |
| 3000 | open 读取字节 (MiB) | 14194.1 | 14284.1 | 0.99× |
| 3000 | open 后常驻 VmRSS (MiB) | 419 | 129 | 3.25× |
| 3000 | 保留池字节 (MiB) | 11465.7 | 11467.0 | 1.00× |

## 结论与边界（按实际测量，不外推）

改善（随档位扩大而拉开）：

- 小修改的读取字节：`edit` 阶段 3.1×→3.4×（3 000 档 974 MiB→290 MiB）。
  基线每次保存读回整份平铺状态参与重编码；新布局按页点读。
- 峰值/常驻内存：`edit` VmHWM 1.5×→2.3×；`open` 后 VmRSS 与 `read`
  VmHWM 2.0×→3.25×（3 000 档 419 MiB→129 MiB）。平铺条目不再常驻，
  读取换成有界页读（1 000 次点查 + 两页共 725 KiB，受 8 MiB 缓存上限约束）。
  基线的"read 读取 0 字节"不是优点：它以全量常驻（419 MiB）换来的。
- 恢复墙钟在大档位开始反超：3 000 档 178 s→133 s（1.34×）。小档位新布局
  更慢（500 档 1.6 s→6.0 s），因为恢复仍完整读取并验证每个命名空间
  页/索引对象（恢复不跳读），对象数多而单对象小，单盘小文件读主导；
  这是预期的验证成本，不是跳过验证能换的。

未改变（在本任务范围外，两实现一致）：

- 保留池字节、`build`/`open` 的总读写字节在三档几乎相同（1.00×）：
  它们由**每版本发布证明**主导（ADR 0004/0006 的既有封装，单证明
  ≤65 节点），与命名空间/日志布局无关。这也是两实现都无法在本容器
  跑 50 000 档的原因（池随版本数超线性增长至 TB 级）。
- `edit` 的写出字节同理被新版本自身的发布证明与正文处理主导（0.97×–0.99×）。
  命名空间的“≤4 个新对象”断言仅覆盖 300 个同目录条目中的一次版本替换，
  不能证明插入、删除或编码工作的界限。原交付编辑器加载并重打包被打开目录的
  全部直接子项，靠对象哈希筛掉未改变的输出；头部插入／删除会改变后续分页。
  该交付未达到局部写目标；后续页局部实现的独立工作量验证见下节。

其他边界：

- 测量在本容器（单盘、共享 CPU、release 构建）得出，不代表物理多盘部署
  吞吐；`wall_ms` 波动较大，字节与峰值内存为主要证据。
- 事件/回执在本负载只有 22 次发布；分段的历史增长效应由上面引用的
  增量编码字节断言与段封印单元测试证明，未在本表以万级历史重复。

## 固定工具链下的页局部工作量验证

[GitHub 验证 40810932267](https://github.com/isarmg/zxs/actions/runs/40810932267)
对源码 `88c2f97721af384a8bdfa18ce973bbeb6026784c` 使用 Rust 1.99.0，实际执行
46 项命名空间回归。以下为 1k／10k／100k 宽目录的首／中／尾位置各三种编辑，
共 27 组记录的逐档最大值，来自同次产物 `namespace-execution.json`：

| 直接子项数 | NamespaceSource 请求次数 | 返回规范字节 | 去重前编码对象数 | 去重前编码字节 |
| --- | --- | --- | --- | --- |
| 1,000 | 5 | 58,329 | 3 | 20,263 |
| 10,000 | 6 | 81,301 | 3 | 33,914 |
| 100,000 | 9 | 120,146 | 5 | 41,115 |

这些是命名空间编辑路径的实际工作计数：请求次数包含缓存命中，返回字节不是设备读取量；
编码在地址去重之前计数。它们不代表 allocator 峰值、磁盘吞吐、锁等待或完整保存延迟。
旧目录快照、工作区选择捕获、名字投影和封段目录复制仍有各自的增长成本，不能据此
宣称整个保存流程为 O(log N)。旧夹具另完成 13 个准确命名空间对象的补录，原有
状态字节、快照／历史对象和根值均未改写；这与上述工作量测试是不同的验证证据。

## 原始测量（JSONL）

```jsonl
{"impl":"baseline","phase":"build","files":500,"wall_ms":10467,"rchar":3492362,"wchar":443052996,"read_bytes":352256,"write_bytes":481820672,"vmhwm_kb":30260}
{"impl":"baseline","phase":"edit","files":500,"wall_ms":2648,"rchar":181856747,"wchar":17850572,"read_bytes":0,"write_bytes":22736896,"vmhwm_kb":52396,"edits":20}
{"impl":"baseline","phase":"read","files":500,"wall_ms":0,"rchar":121,"wchar":0,"read_bytes":0,"write_bytes":0,"vmhwm_kb":86052,"lookups":1000,"found":1000,"page_len":6,"events":22}
{"impl":"baseline","phase":"open","files":500,"wall_ms":1606,"rchar":406554581,"wchar":168928,"read_bytes":0,"write_bytes":200704,"vmhwm_kb":61016,"commits":22,"vmrss_kb":61016}
{"impl":"baseline","phase":"pool_size","files":500,"bytes":348991348}
{"impl":"baseline","phase":"build","files":1500,"wall_ms":58170,"rchar":10372651,"wchar":3951788568,"read_bytes":581632,"write_bytes":4072665088,"vmhwm_kb":65220}
{"impl":"baseline","phase":"edit","files":1500,"wall_ms":5511,"rchar":512675835,"wchar":50298702,"read_bytes":0,"write_bytes":56094720,"vmhwm_kb":126588,"edits":20}
{"impl":"baseline","phase":"read","files":1500,"wall_ms":0,"rchar":118,"wchar":0,"read_bytes":0,"write_bytes":0,"vmhwm_kb":229360,"lookups":1000,"found":1000,"page_len":16,"events":22}
{"impl":"baseline","phase":"open","files":1500,"wall_ms":6296,"rchar":3196476773,"wchar":461448,"read_bytes":0,"write_bytes":520192,"vmhwm_kb":153900,"commits":22,"vmrss_kb":153900}
{"impl":"baseline","phase":"pool_size","files":1500,"bytes":3033598850}
{"impl":"baseline","phase":"build","files":3000,"wall_ms":239497,"rchar":20821170,"wchar":15769663167,"read_bytes":8155136,"write_bytes":16045477888,"vmhwm_kb":119196}
{"impl":"baseline","phase":"edit","files":3000,"wall_ms":17351,"rchar":1021177008,"wchar":96325964,"read_bytes":20480,"write_bytes":103317504,"vmhwm_kb":244492,"edits":20}
{"impl":"baseline","phase":"read","files":3000,"wall_ms":1,"rchar":131,"wchar":0,"read_bytes":0,"write_bytes":0,"vmhwm_kb":428716,"lookups":1000,"found":1000,"page_len":31,"events":22}
{"impl":"baseline","phase":"open","files":3000,"wall_ms":178422,"rchar":14883623134,"wchar":5200408,"read_bytes":9735815168,"write_bytes":36544512,"vmhwm_kb":428672,"commits":22,"vmrss_kb":428672}
{"impl":"baseline","phase":"pool_size","files":3000,"bytes":12022634523}
{"impl":"paged","phase":"build","files":500,"wall_ms":10928,"rchar":3482141,"wchar":442295275,"read_bytes":28672,"write_bytes":482357248,"vmhwm_kb":30564}
{"impl":"paged","phase":"edit","files":500,"wall_ms":2745,"rchar":58877786,"wchar":18462711,"read_bytes":0,"write_bytes":23142400,"vmhwm_kb":34628,"edits":20}
{"impl":"paged","phase":"read","files":500,"wall_ms":8,"rchar":147097,"wchar":0,"read_bytes":0,"write_bytes":0,"vmhwm_kb":43184,"lookups":1000,"found":1000,"page_len":6,"events":22}
{"impl":"paged","phase":"open","files":500,"wall_ms":6025,"rchar":482093404,"wchar":168928,"read_bytes":0,"write_bytes":200704,"vmhwm_kb":39224,"commits":22,"vmrss_kb":39224}
{"impl":"paged","phase":"pool_size","files":500,"bytes":349644309}
{"impl":"paged","phase":"build","files":1500,"wall_ms":62730,"rchar":10326459,"wchar":3944977639,"read_bytes":12288,"write_bytes":4089585664,"vmhwm_kb":64288}
{"impl":"paged","phase":"edit","files":1500,"wall_ms":5006,"rchar":151956615,"wchar":51026806,"read_bytes":0,"write_bytes":57348096,"vmhwm_kb":76204,"edits":20}
{"impl":"paged","phase":"read","files":1500,"wall_ms":17,"rchar":385288,"wchar":0,"read_bytes":0,"write_bytes":0,"vmhwm_kb":100000,"lookups":1000,"found":1000,"page_len":16,"events":22}
{"impl":"paged","phase":"open","files":1500,"wall_ms":13099,"rchar":3386914117,"wchar":457328,"read_bytes":0,"write_bytes":516096,"vmhwm_kb":88292,"commits":22,"vmrss_kb":88292}
{"impl":"paged","phase":"pool_size","files":1500,"bytes":3034313521}
{"impl":"paged","phase":"build","files":3000,"wall_ms":220104,"rchar":20708510,"wchar":15742518150,"read_bytes":8491008,"write_bytes":16023101440,"vmhwm_kb":116332}
{"impl":"paged","phase":"edit","files":3000,"wall_ms":14221,"rchar":303874425,"wchar":97867443,"read_bytes":4096,"write_bytes":105140224,"vmhwm_kb":106384,"edits":20}
{"impl":"paged","phase":"read","files":3000,"wall_ms":24,"rchar":742301,"wchar":0,"read_bytes":0,"write_bytes":0,"vmhwm_kb":132116,"lookups":1000,"found":1000,"page_len":31,"events":22}
{"impl":"paged","phase":"open","files":3000,"wall_ms":133343,"rchar":14978008924,"wchar":5204528,"read_bytes":9823326208,"write_bytes":36630528,"vmhwm_kb":132056,"commits":22,"vmrss_kb":132056}
{"impl":"paged","phase":"pool_size","files":3000,"bytes":12024040631}
```
