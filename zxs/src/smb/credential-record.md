# 内置 Rust SMB 的私有凭据正本 v1

`credential.v1` 是 **240 字节**私有记录，不是网络包、SQLite 行、公开授权票，也不接受既有安装的账号导入。v1 只指定一个本地认证域 `ZXSLOCAL` 及本项目现有严格 NTLMv2 策略；安装身份的 SPN、地址与机器名仍须来自实际受信安装。

所有区间从 0 开始、右端不含。ID 保留原始 16 B，不作字符串/GUID 字节换序。

| 区间 | 字段 |
| --- | --- |
| 0..16 | ASCII `ZXS-RUST-SMB-V1`，随后一个 NUL，指定唯一版本 |
| 16, 17, 18..24 | SMB role = 1；登录名长度 1–64；六个零保留字节 |
| 24..40, 40..56, 56..72, 72..88 | PoolId、InitOp、SetupOp、InstanceId |
| 88..104, 104..120, 120..136, 136..152 | Rust 安装 BindId、SMB AccountId、CredentialOp、VaultRecordId |
| 152..160 | 正 generation，大端无符号 64 位，至多 i64::MAX |
| 160..224 | 登录名规范小写 ASCII（沿用现有文法与 SMB 禁用名），其余零填充 |
| 224..240 | 仅 [ResponseKeyNT](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nlmp/5e550938-91d4-459f-b67d-75d70009e3f3)，密码等价秘密，全零不是有效记录 |

所有 ID 均非零。InitOp 与 SetupOp 不同；第一代 CredentialOp 必须等于 SetupOp，后继不等于 InitOp 或 SetupOp。真实角色、前驱/代、确切原批准与 ACTIVE 必须另用同一原 SQL 截面核对。BindId 来自新 Rust 安装，VaultRecordId 由实际维护者每代单独分配并在原待办冻结；解析器不证明其来源，不能以这些公开坐标、摘要或历史回执代替原件及授权。不存明文、NT hash、SID、秘密摘要或备用 key。

解码只接独占的定长清零缓冲区、返回同样受控的未资格材料；任一失败只回不带字段或秘密的拒绝。不保证擦除编译器、内核或其它副本。格式解码器不负责打开文件、发会话或提供一般的日志、复制、取 key、序列化入口。原[站点启动](launch/site/CONTRACT.md)在原安装锁下核同一受保护目录、只读原 FD、确切字节及紧接 EOF，再与同一原 ACTIVE 记录核对；**能解码或字段吻合均不代表通过**。不接受旧安装记录、旧凭据或回收卷做这项证明。

固定的 `src/test/smb/credential_record.rs` 只用公开捏造 key 和同域固定报文，测试专属代码才把这样的解码材料交给现有严格 Type 1/2/3 验证。没有实际凭据、构建或运行测试。


## 原安装资格与轮换顺序

公开的 `rust.install` 不是凭据。它固定原 Pool/INIT/Instance/Bind/SETUP、唯一批准主机名和各具体监听/客户前缀、原普通盘及目录/初始模式、单个回收标签。它是这一个站点的公开计划，不是可替换的运行配置，不能用旧安装记录或摘要代替。V1 起始文件使用 `ZXS-RUST-SMB-I1\0`（16 B）和上述 5 个原始 ID（该顺序）。后接 u8 长度的唯一小写 ASCII DNS 主机名（1–253 B，每段 1–63、无首尾连字符；首段至多 15 且含字母；允许单段，不接受地址字面量或通配）。之后是传统原始端点及名称体的界限，不借旧 magic、凭据或身份。精确 EOF，上限 17,885 B；发布/ShareId/首个模式 ID 按 `SHA256("ZXS-RUST-SMB-I1/ID\0" || SHA256(full descriptor) || kind:u8 || volume:16)[..16]`，kind 自 1 到 3 分别为 ShareId、发布、初始模式。实际当前模式仍由原 Core 设置重核，不将初始批准为永久写权。

SQLite 只固定 **不可变安装绑定和非密坐标**。同一库中没有第二账号/代/活动状态；此在线路径只消费 Rust 绑定，要求原 BindId 和上述完整描述符 SHA。只有原 Rust SETUP/RESET（kind 2/7）各携唯一非密 record ID，仍由同一原 `active_credential` 按角色选择。新 PENDING 令旧 SMB 代失活（不更改 Web），无回退到最近已提交一代。首设/重置分别使用独立请求摘要域，便于另一实现精确核对，而非“用摘要认证客户端”。新安装只认完整精确 DDL；不迁移旧库。不以其他安装类型或历史回执作后备。

启动保留原 S SH 到同一物理实例目录 EX；离线审批持原 S EX 后取得**同一**实例目录 EX，确认与已退出的协议服务互斥。没有 Rust 专用的第二把站点生命周期锁。长期服务入口必须持实证而非只传 path/UUID/摘要，不接手已有的旧实现实例，也没有配置切换到第二类 SMB 后端。

离线顺序：同一站点唯一固定 ACTIVE 文件位于 `vault/credential.v1`；新值只写本次受控、不可用于登录的独占 staging inode。只读重开实际具名文件、同步文件和原目录，原 SQL 预留 PENDING 后独立重开确认同一次；再在同一原 vault 上按 exact predecessor 安全换位，文件/目录再同步、独立重开，才允许真低权 SQL 写方提交 ACTIVE。首设 ACTIVE 与所有批准原始 F 发布/初始设定同一 SQL commit。任何崩溃、替换、晚通知或记录不符均保留原待办、失败关闭；不能清未知文件、重做其他申请或用失效代继续登录。只将受保护的只读实际活动 inode 交给同次运行；相关证据、记录原字段与 Core 的原 ACTIVE 同切重复比较，仍需协议证明才有会话，且会话不是文件访问权。
