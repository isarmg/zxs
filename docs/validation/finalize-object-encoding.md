# FINALIZE 对象编码复用

正常发布以及尚无 PUBLICATION 的恢复完成路径，在全部成员 ACCEPT 收据收齐后，
以既有 `PreparedMetadataObject` 冻结同一份 `Finalize` 的规范字节和摘要。
此对象包含全部成员的收据，不是当前循环成员的局部视图；循环中不会改变它。
每个成员仍按原顺序先安装不可变对象、再写入并同步该成员的 FINALIZE 日志。
不先批量写完所有对象，也不共享各成员的实际落盘回执。

对象写入沿用持有盘根、相同字节碰撞核验、大小上限和文件／目录同步。
全部成员结束后释放临时编码缓冲；嵌入后续本盘证明及 PUBLICATION 的领域对象仍保留。
没有跨请求缓存、格式变化、恢复豁免或同步次数减少。

## 可复核的成本边界

原路径先计算一次 `object_hash`，每个成员的 `store_object_at` 又分别为摘要和
文件字节各编码一次。M 个成员时，该独立对象共编码 `1 + 2M` 次；现在为一次。
每次规范编码仍包含整数表示验证和实际序列化，不能把一次编码说成只调用一次
`Serialize`。带 M 份收据的对象本身仍为 O(M)，M 份独立持久副本的字节写入未减少。
这不是端到端吞吐、物理 I/O、锁等待或硬件时延测量。

## 验证入口

`journal::replication_tests::prepared_finalize_preserves_all_accepts_with_one_encoding`
使用真实三成员仓库发布生成的 Finalize，检查各成员 ACCEPT 的身份和日志绑定，
再对照原存储算法与准备对象路径的摘要、持久字节、解码值以及序列化调用数。
三成员对照断言为 14 次调用到 2 次，不在生产代码加入计数 hook。

既有 `prepared_publication_objects_encode_once_across_individual_held_member_stores`
同时覆盖 candidate、finalize 和 publication：逐成员安装后其余成员尚无对象，
重复安装不重新编码，损坏不可变对象仍报告碰撞且不覆盖原字节。
`typed_phases_survive_normal_publication_and_recovery` 覆盖实际四阶段及重新打开恢复。
以上是测试定义；具体执行结果须以对应提交的运行日志和 CI 为准，不能从本页推定通过。
