# 显式登录，只查原回执

[仅静态] Linux 候选增加只查回执的独立模式。服务端必须支持[第一阶段原回执会话](../../../zxs/docs/specs/sessions.md)。此候选没有替换默认入口，未完成真实 HTTPS/2 安装验收。

用已有私用工作区、相同 HTTPS 地址和账号，明确加上 `--receipt-session`：

```sh
zxs-client --receipt-session HTTPS地址 账号 /绝对/私用目录 reconcile
zxs-client --receipt-session HTTPS地址 账号 /绝对/私用目录 status 原操作ID
```

先核本机原工作区、地址、账号和系统信任；验证同一条 HTTPS/2 信道后才提示输入口令并登录。该模式登录成功后只允许原回执 GET，精确匹配时可核对并记录本机原结果。查一个 ID 不会先查询其他待决。

- 普通 `open` 仍完全离线。只查回执不需要原正文；缺少回执也不会自动发送。
- 登录失败、令牌到期或查回执被拒，不切到旧 Basic 业务模式，也不自动续登。首次 Basic 授权只给登录入口；令牌只在本次进程使用。
- 列表、另存和明确提交仍走原 Basic 命令，不能用本模式代替。服务支持不代表真实代理或发行资格通过。

[原号与恢复边界](../invariants/receipt-recovery.md) · [验收状态](../../TASKS.md)。
