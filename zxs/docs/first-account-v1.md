# 首次设置：两个固定账户，一笔 SQLite 事务

每个完成首次设置的池只有一位 Web 管理员和一位 SMB 专用用户。两者的用户名、稳定 AccountId、凭据/代次和准用协议分别绑定。不创建普通用户，不设逐人 ACL；无初始密码。任何时候都不接受匿名、guest、空口令、未知用户或认证失败后的回退。

### 初设与非秘密原回执

只有独立 helper 才能在服务已排空、可信原 S 目录全程 EX、已确认 INIT、从未设置两账户且**另有该次维护授权**时签 `SETUP_PAIR=2`。首先查重；原请求按下列确切字段的原字节计算 SHA-256（域末是一个实际 NUL），不是口令验证方法：

    "ZXS-SQLITE-1/SETUP-PAIR\0" | PoolId[16] | InitOp[16] | SetupOp[16]
    | offline_actor[16] | approval_ref[16]
    | web_len:u8 | web_login | smb_len:u8 | smb_login | smb_sid[28]

两登录名各 1…64 字节、小写 ASCII 字母/数字/点/下划线/短横，首字节限字母或数字，不同名。SMB 禁用 guest、anonymous、nobody 和 root。原本机 Samba 认证域的规范用户 SID 固定为 S-1-5-21-x-y-z-1000：28 字节、原 12 B 非全零域号、RID 1000，不是内建管理员/来宾。部署需独立认出同一受控本机域，不能接受客户端选域、同名 SID 或重映射。允许其他本机正当进程拥有 OS 账号，但它们不因此成为 ZXS 用户。Samba 是否能正常登录不能由 SID 或密码材料的长度推出。

一笔 `BEGIN IMMEDIATE` 中按[唯一 DDL](../src/system/schema.sql)先插 `account_pair(singleton=1,birth_op,approval_ref,web_id,web_login,smb_id,smb_login,smb_sid)`，再插两行：
- `web_password(slot=1,credential_op,generation=1,salt,verifier)`：Web 密码只用上游 Argon2id v19，64 MiB、3 次、4 lanes，16 字节随机盐、32 字节验证值。输入按实际 UTF‑8 字节，不截断、不规范化，非空至多 1024 B。
- `smb_password(slot=1,credential_op,generation=1,nt_owf)`：独立强密码用于受控、独立运行的 Samba standalone/NTLMv2；仅保留成熟上游生成的 16 B NT 单向函数结果。它是**密码等价秘密**，不保 LM、明文或口令历史；不以 Web Argon2 结果衍生。上游受保护输入及临时 UTF‑16/密钥缓冲用后显式清除，不经命令行、环境或普通临时文件。禁空口令或全零材料。上游不能安全提供所需实现或宿主禁用 NTLMv2 时该入口关闭，不降级。

最后插入同 Op 的 `operation(kind=2,state=1)` 并 COMMIT。两条凭据的 credential_op 初始均等于 setup Op；它们是不同角色的首代，不是可互用的会话。两个不同的 AccountId 只签一次。该授权、随机结果和两种密码生成不在长 SQL 事务内；进入短事务时重新核真实 S、原池/首设资格与维护授权。本 DDL 不允许一半账户组提交、OR REPLACE、改名或多设一组。`operation` 与[冷查 SQL](../src/system/original-first.sql)只含非秘密原请求/原结果；丢回复时带同 Op、主体、批准、池/原 INIT、两名称与 SID 查回两原 ID，不比用户带来的口令，不重生盐、材料、代次或账户。未知结果先查；从未提交且原授权/上次准备无法证明时，停止而非借旧号重设。[固定向量](../tests/sqlite/first-account-v1.txt)含 154 字节原请求的摘要；SMB 字段为结构用合成字节，不是 SMB 认证样本。

### 协议和凭据边界

Web 的内部 `authenticate_web` 只取 Web 慢验证值与非秘密首设数据，放开 SQL 事务再调用 [libargon2](https://github.com/P-H-C/phc-winner-argon2/blob/20190702/include/argon2.h)，随后新短切面核同一原账户、凭据及原 S，才给 Web 管理身份。进程内限制 KDF 并发不替代网络限流。库是目标系统真实动态链接依赖；离线发行须提供审准的 [目标 ABI](https://packages.debian.org/trixie/amd64/libargon2-1/filelist) 和[许可](https://github.com/P-H-C/phc-winner-argon2/blob/20190702/LICENSE)。管理者可定义普通根共享，不开 FILE。

SMB 网络接入须用真正的 [Samba SAM 验证](https://github.com/samba-team/samba/blob/samba-4.22.6/source3/auth/check_samsec.c)和[受控配置](https://www.samba.org/samba/docs/4.22/man-html/smb.conf.5.html)。规划仅让指定的本机只读 passdb 提供者取唯一 SMB 用户的 SID/当前协议验证材料；不建立第二个 tdbsam/smbpasswd 账户密码库，不在 VFS 自写握手或向浏览器开放此字段。SQL 普通访问仍由可信进程和私有目录约束，不能把同库分表宣传成 OS 级秘密隔离。必须证明真实会话及其当时凭据代次、无 guest/anonymous/null 映射，且普通 UNIX 身份、凭据代次数字或一个自称 SID 均不能颁发 SMB 句柄。未具备这些条件，没有 SMB 登录/文件入口；不把测试夹具当实际 Windows 认证。

忘记密码的重置由独立 helper 在本机原 S 的 EX 下另获恢复授权，只绑定其中一类固定角色、原 PoolId/AccountId、原凭据 Op 和准确代次；另一个角色不变。已知密码的日常更改如后续实现，必须有明确授权，不能借 Web 会话代替离线恢复权限。新动作短事务替换当代秘密、仅一次递增代次和写非秘密原回执；旧会话、缓存/耐久句柄不得在重连时升级。需在同一 SQLite 权威中确实轮换 SMB 提供者读取的原材料、排空旧登录/缓存/重连，旧 SID 重发不是证明。**当前首版没有重置发行、Samba 提供者或 Web 监听**，以触发器拒绝变更而非开放半条路径。
