# 单一 SMB 账户的离线密码重置

只适用于已经由原 ZXS 安装记录认证的 v17 站点。先按现有方式让原
ZXS 完全退出，再用独立可信发布信息核对本次 Helper 的完整 SHA-256。
Helper 不停止或启动 ZXS，不迁移池，不重设 Web 管理员、历史权限、共享或卷。
原 SYSTEM 或站点被占用、缺失或换过，命令拒绝，不另找站点。

`helper check-deployment` 只读核对当时的原 SMB 操作 ID 和代次。
记录它们，作为新操作的 `--previous` 和 `--previous-generation`。
在同一真实前台终端明确选择一个新的操作 ID 及批准参考，各为 32 位小写
十六进制字符。供应商摘要同样来自独立可信材料，不能由不明文件自行批准。
新模式需要部署中受保护的固定 `libgcrypt.so.20`、`libgpg-error.so.0`
实际完整摘要及系统 crypto policy；只读回执不加载这两个库。

```text
helper reset-smb new --op NEW_OP --approval APPROVAL --previous CURRENT_OP --previous-generation N --helper-sha256 HELPER_SHA --gcrypt-sha256 GCRYPT_SHA --gpgerror-sha256 GPGERROR_SHA
helper reset-smb receipt --op SAME_OP --approval SAME_APPROVAL --previous SAME_PREVIOUS --previous-generation SAME_N --helper-sha256 HELPER_SHA
```

新操作先在实际原 SYSTEM 排他租约下核原 INIT、受保护站点、原 Rust
SETUP/Bind/描述摘要、当前 SMB 代次及唯一具名 vault 原件；只读旧
vault 的 224 字节公开前缀，不读旧密码等价材料。原前序变化、现行代次
未决、其他旧名字、此次原操作已经存在或核验不完整都拒绝。终端预览
显示确切前序、目标代次、站点、账户、操作和原申请摘要。单次
`RESET-RUST-SMB` 挑战获明确确认后，同一原终端再隐蔽输入新密码两遍；
密码不进入 argv、配置、数据库、进程间回执或报告。Web 凭据不读取或重置。

只有当前真实低权限同一程序读取新密码；受保护的现用原件先继续保持，
新密码材料写到同一 vault 内本次独占创建且已验证的暂存原件。现用与
暂存的确切实体在末端再核；暂存文件和原父持久化且 SQLite 重新确认
只属于此操作的 PENDING，才以同一暂存原件原子替换原已验证
`credential.v1`。低权 SQLite 再提交并独立重开确认新代次，
同时保持同原 SYSTEM、卷标、SQLite 锚和新 vault 证明。不是
单方写一个新文件就算成功。

如果中断或结果不明，保留现场，只用原 ID、批准参考和原前序查
`receipt`。精确已确认的回执是历史决定，不是当前密码测试；能证实
PENDING 时只报未完成，不回退代次，也不重发新请求。没有精确回执
不代表可重试、可换盘或可删除材料。此切口没有自动或手工续作、强制
清理、猜测的修复或在旧操作上再次输入密码；未决代次可能暂时阻止原
部署冷启动。在开发阶段的合成 SQL、进程间协议和伪终端用例不是
现网重置、登录或恢复验收。
