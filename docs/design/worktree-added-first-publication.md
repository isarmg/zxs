# 单 Added 的首次 FILE 与目录发布

## 范围

原 `CommitWorktree` 仍提交整个实例，没有新选集。本片仅允许一个 Added、没有原文件稀疏
编辑、命名空间修改或已发布 FILE 义务。其他组合在原操作保留之前拒绝，不宣称多文件提交。
已有单个原文件编辑仍沿原路径。普通 Flush/Close 不进入本链。

## 先固定原操作，再准备真实 FULL

新增 `ReserveAddedCommit` 保留原请求、完整 Added target/birth/working、实例 checkpoint、
精确原 snapshot、当时真实 HEAD、目录路由及独立子操作身份。源仍为 HEAD 则续原分支；
否则分配目录分支，显示名使用原实例名称。FILE 本来没有历史，因此创建独立 FILE main。

`WorktreeFirstSource` 私有发行点核对原 reservation、当前实例独立收据和真实 deleted-data
容器，持有 State Arc、Config、resources、slot 与不可变 extent reader。既有
`PreparedUnbornFull` 只扩输入来源和一个共同准备入口，不复制合成算法或新增 writer。
它实际合并区间、写独立 FULL inode、完整 readback，再让标准 Version 构造器和 native
SYSTEM/DISK/FILE publisher 安装另一份不可变 Version。整个正文读取只在显式提交发生。

原 reserve 后取得该 producer，producer 与首次 FILE 决定之间不插入第二个 reserve。
真实 Version 身份产生后，FILE 持久决定前预查两帧和 ledger 总预算。`PublishAddedFile`
冻结完整构造结果与实际 FULL 身份，其原 native Version protection 明确为 TargetMet。
已有 metadata 原记录仍是空 protection，原编码不改，也不提供默认字段或迁移读取。

## FILE 归属与目录材料

共用现有首 Version 重建和 FILE root 初始化核心，但不调用普通
`publish_first_namespace`。原 ordinary main、路径 context、位置和其他实例的稿不变。
容器的 Worktree parent 永远锚定原 birth，首次发布不移容器到 common-data；后续实例
引用该 FileId 也不改容器归属。冷准入通过 birth + 首 FILE 原回执核验这一 retained parent。

首次 FILE 已独立发布后，索引核验确切 Version/FILE branch/commit、主体和子原操作，再将
真实热 FULL holder 交给原目录 node publisher。它 MOVE 独立 FULL，不搬 FILE Version；
所有未改原 snapshot 正文仍 COPY。`AddedFileResult` 保留 EntryId、FileId、原 birth、
所消费 working 和实际 FILE result，不伪装成 source manifest 的原 occurrence。

FILE 发布后 State 已推进。MOVE 只消费仍持有的原 inode/reader 和独立 FILE 收据，不把旧
`require_context` 对新 State 强行判成成功。序列化的 FULL id/entity/hash 没有热重建入口。

## 完成与重试

CommitAdded → 实际首 FILE → 实际完整 node → AddedCompleted → 独立 SYSTEM completion。
普通读取只认最后独立决定；中途失败保持原 Pending，不换 operation、另建容器或猜正文。
真实 node 已发布时，复用原两帧 tail 准入收尾；更早失败不新增自动续作机制。
准确原请求冷重试只返回原 handle，历史、正文和其他实例稿不重复改写。

公开 HTTP Commit/原结果查询契约不变。已提交 Added 的 metadata 使用 RetainedFile，
准确保留实际 EntryId，并读取真实 published node，不能继续冒充 Unborn 工作稿。

## 验证口径

新增真实 Engine fixture 核 FULL 实际 inode 被 MOVE 到 node、FILE Version 副本 inode 不同、
未改项 COPY、原 ordinary 分支和容器归属不变、其他实例稿保留、从新 node 开另一实例以及
冷开原请求零写重试。同批 server fixture 覆盖真实 DefaultShare native → 认证 TCP Commit。
候选提交者只做静态语法及补丁准入，编测由单目标集成执行；未运行外部 Samba 网络会话。
