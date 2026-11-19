# SQLite 首管、原操作与口令验证

新池只认[同一份 SQLite SYSTEM](sqlite-system.md) 与[唯一 DDL](../src/system/schema.sql)。ZXS 在原 SYSTEM 目录长 SH 内只读已确认的首管；旧首设帧不再是生产账户源。不提供网络登录、账户写端、共享或 FILE 权限。

**原动作。** 已完成 INIT 的池最多一次 `FIRST_ADMIN=2`。独立 helper 必须取得原 S 的离线 EX，另行验证维护主体与本次一次性的首设授权，先查完整的原池、历史与原单。不能凭「没有活跃管理员」、UID、账号名字、审批关联号或进程已停机授权。管理档必须明确选 1（管理读 ACL）或 2（管理读及写 ACL）；两者都不自动创建共享或任何 FILE 读写权，包括管理者自己。

**原请求与回执。** 按 `"ZXS-SQLITE-1/FIRST-ADMIN\0" | PoolId[16] | InitOp[16] | FirstOp[16] | offline_actor[16] | approval_ref[16] | profile:u8 | login_len:u8 | login` 计算 SHA-256，域末是一个实际 NUL。登录名按 1…64 字节精确比较：小写 ASCII、数字、点、下划线、短横线，首字符字母或数字。AccountId 是一次生成的结果，不是重试输入；明文口令、盐、验证标签和可快速猜口令的摘要都不写进原请求、日志或回执。[固定向量](../tests/sqlite/first-account-v1.txt)是两个不同首设选择，不能在一池设两次。[独立维护查询](../src/system/original-first.sql)只在当前获准披露时使用，逐项核原请求、签发主体、批准关联和确切池，返回当时 AccountId 与管理档。不会比较查询者给的密码，也不会产生新密码或会话。

**一笔事务。** 新首设只生成一组稳定 AccountId、随机 16 B 盐和 32 B Argon2id 验证值（密码按实际 UTF-8 字节 1…1024，不截断、不规范化，不走 argv/env）。计算在 SQL 事务外。重新核原授权与原池后，在一个短 `BEGIN IMMEDIATE` 先排除任何相同 Op 或既有 FIRST_ADMIN，依序插入：
1. `account(account_id,birth_op,login,approval_ref,boot_profile)`；
2. `account_password(account_id,credential_op,generation,salt,verifier)`，首个 credential_op = birth_op、generation=1；
3. `operation(op_id,kind,actor_id,request_sha,state)`，kind=2、state=1，然后 COMMIT。

这不是外部 I/O 的 pending 操作。两个[延迟外键](https://sqlite.org/foreignkeys.html#fk_deferred)使缺原单的账号/凭据不能提交；原单亦须在同一账号、凭据齐全后插入。禁止 OR REPLACE 与静默重设。COMMIT 结果未知必须从同一可信库查原单；已提交只返原结果，不重生 ID、盐或凭据。确切未提交时不存在已发行 ID/盐；若原候选或授权已丢，先停止，不把一次查询或不明密码当成再次设密的许可。数据库不证明离线审批或当时口令输入的真实性。重置/禁用/其它账户还须各自明确的操作、授权和凭据代次，不是修改这段行的便捷接口。

**在线核验。** `InitialReader::authenticate_first` 在同一真实 SQLite 短事务内核当前 schema、已完成原 INIT、原 S 标、首管、管理档和原凭据；事务释放后核口令，再在一个新短事务比较同一组原件。确认的内部主体只可重新获取自己的管理档，不是授权布尔值或对外令牌；不提供 SMB/FILE 权、共享、监听或 Web 文件接口。旧便携账户/成员解析仅留历史测试。此版本没有构建、运行 SQL/KDF/测试或实现双方实盘互通。

**依赖。** 使用目标 Linux 的成熟 [libargon2](https://github.com/P-H-C/phc-winner-argon2/blob/20190702/include/argon2.h) 的 `argon2id_verify` ABI，固定 v19、64 MiB、3 次、并行度 4，交由库比较验证标签；不从请求接受 PHC 或算法参数。进程内并发上限 1，不能代替网络端未来的限流。构建 sysroot 需有目标架构的 [libargon2.so](https://packages.debian.org/trixie/amd64/libargon2-dev/filelist)，离线发行须随附已核准的 ABI 库和适用许可，或要求预装等价 [libargon2.so.1](https://packages.debian.org/trixie/amd64/libargon2-1/filelist)。不承诺单文件，不自动安装，无自写/降级算法。上游[许可](https://github.com/P-H-C/phc-winner-argon2/blob/20190702/LICENSE)为 CC0-1.0 或 Apache-2.0。向量中唯一的示例盐、验证值与输入来自 [argon2-cffi 25.1.0 公开示例](https://argon2-cffi.readthedocs.io/en/stable/howto.html)，不代表任何用户或默认账号。
