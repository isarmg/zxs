# 通用介质：同一原登记上的第一个真实决定

这是内部只读范围：从调用方明确选择的 SYSTEM 和预期 PoolId 观察已确认的首个账户原结果。既没有新写入口、账户会话或默认共享，也不以首次管理员资格授权 SMB、CREATE、SAVE 或后台任务。原 [便携 GENESIS](portable-storage.md) 与双方的初始卷标、零下界逐字不改。

- `REGISTRATION` 终身不替换，生命周期 SH 先锁原普通常规文件，短暂锁同一 S 后读取；验证第 0 行的真实 S、当前 storage UID、组件预算、原 103 B 卷标以及同切面的 `CONFIRMED`。此观察不跟随任何其他卷路径，不要求 R/F 在线；不可辨的整池副本不自称唯一原件。目录只容许原 `REGISTRATION`、`CONFIRMED` 与原卷标三个控制项。整条前缀一旦变化，旧观察不能自行续权，须重开原切面。
- 所有整数大端。`B||G` 之后逐个决定：`"ZXS-PORTABLE-1/DECISION\0" | total:u32 | pool:16 | seq:u64 | previous:32 | kind:u8 | OpId:16 | request:32 | body | D:32`。total 含魔数与末尾摘要，最小固定外壳 165 B，每帧最多 4096 B。D 是本帧自魔数至 body 末字节的 SHA‑256，不是含末尾 D 的二次散列。第 1 帧 previous=G；后续必须为直接原前一帧 D。完整下界照原协议为 `"ZXS-PORTABLE-1/CONFIRMED\0" | G | seq:u64 | GENESIS 之后累计帧字节:u64 | head:32`。共用容量为 8192 笔、8 MiB 帧字节；不是版本代数上限。
- 当前仅受理严格空切面，或恰好第 1 笔 kind=9 的首管建立。typed body 为 `AccountId:16 | operator_ref:16 | login_len:u8 | login | salt:16 | argon_tag:32`，OpId 已在信封内，不重复。复用[原首管语义和 KDF 约束](first-account-v1.md)；request 仍为 `SHA256("ZXS/FIRST-ADMIN/REQUEST/1\0" | pool | OpId | AccountId | operator_ref | login_len | login)`，没有口令、盐或验证值。账户前沿仅证明该笔原结果，不证明看见 operator_ref 的人有维护权。旧族的 `FILE-DECISION` 不回退解读。
- 只承认完整且被这个下界准确确认的前缀。部分首帧、帧已写而仍为零下界、任一额外（即便完整）未确认帧、未知种类、陌生侧件、重复首设、旧前缀替原件均拒；不得返回上一次成功结果冒充现在。私有接口目前只按原 Op 查询非秘密账户身份与原决定摘要，不吐 salt/tag，也不接受口令作为查询条件。后续实际授权、FILE 出生/放置和自动 SAVE/任务需在这一个原决策流中衔接；可解卷标、匿名句柄、单独的物化读回及首管角色均不发 FILE 权限。
- [静态独立向量](../tests/vectors/portable-first-decision-v1.txt) 接便携四根 GENESIS，使用既有公开合成的账户字段，frame=257 B，floor=105 B。它不是执行记录；helper 此时仍只独立处理其支持的空池阶段，没有因此签、验或恢复账户，也不能把带决定或未知尾的盘当作空池。
