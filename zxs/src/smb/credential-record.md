# Rust SMB 凭据正本格式 v1（未接生产）

`credential.v1` 是 **240 字节**私有记录，不是网络包、SQLite 行、公开授权票或 Samba TDB 替代导入。v1 只指定一个本地认证域 `ZXSLOCAL` 及本项目现有严格 NTLMv2 策略；安装身份的 SPN、地址与机器名仍须来自实际受信安装。

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

解码只接独占的定长清零缓冲区、返回同样受控的未资格材料；任一失败只回不带字段或秘密的拒绝。不保证擦除编译器、内核或其它副本。没有一般日志输出、复制、取 key、序列化或生产会话的入口，当前不打开任何文件。真实 launcher 须以后在原安装锁下验证保护目录、只读原 FD、确切字节及紧接 EOF，再与同一原 ACTIVE 记录重新核对；**能解码或字段吻合均不代表通过**，不接 v9 Native BOUND/TDB，也不涉及 R。

固定的 `src/test/smb/credential_record.rs` 只用公开捏造 key 和同域固定报文，测试专属代码才把这样的解码材料交给现有严格 Type 1/2/3 验证。没有实际凭据、构建或运行测试。
