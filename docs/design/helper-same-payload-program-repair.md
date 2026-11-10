# 内置 ZXS：同载荷程序修复，保留数据与配置

本片原始源码基线 `5c14a7a9`，接续已有内置 ELF 首装，不增加运行时下载或第五类发行资产。**未审查、编译、格式化、测试或运行 CI；没有生成实际发行载荷，没有执行 helper 或操作真实主机。** 新增 13 项测试源码，不等于已验证修复。

## 严格范围：原安装的同一载荷

helper 继续携带受控构建入口先生成的准确 ZXS ELF，版本、Git commit/tree、目标、长度和 SHA-256 绑定不变。普通开发构建没有载荷，首装与修复均拒绝；不从 PATH、旁边文件、网络或浏览器上传寻找替代品。

首装现在先持有已存在 `/zxs` 的当前 daemon 生命周期锁，再持有程序父目录锁。准确安装/核验完成后，在 `/zxs/.zxs-helper-program-baseline/receipt.json` 保存不可变的载荷身份、固定目的地、主机根和程序父目录 device/inode。文件 root 所有、0600，目录 0700，沿用 no-clobber bundle 与同步屏障。

- `/zxs` 必须先由现有 host 步骤准备；首装不隐式创建它。
- 已有准确程序、无基线时，普通首装可重新核验字节并建立当前基线；不同程序仍绝不被首装覆盖。
- 已有不同载荷/父身份的基线直接拒绝，不能通过新 helper 覆写它进行升级或降级。
- 基线目录已开始却无可读 receipt，或字节损坏/不匹配时拒绝保留，不猜测原版本。正常中断仍可能需要明确调查，并非所有损坏材料都能自动恢复。

**修复必须已有这份准确基线。** 没有基线的旧安装/未知版本不自动收养为可修复系统。它不能证明外部管理员从未绕过工具运行其它版本；停止并排除外部程序管理者仍是操作前提。工具不探测或转换运行池格式，不对数据兼容性或整个系统可启动作承诺。

## 用户入口

```text
zxs-helper install repair-preview
zxs-helper install repair-commit --request-digest <准确预览摘要> \
  --confirm-same-payload-program-only --external-administration-stopped
zxs-helper install repair-recover-preview --request-digest <原摘要>
```

独立 Web 的程序安装卡片另设同载荷修复区域。HTTP POST 为 `/api/install/repair-preview`、`/api/install/repair-recover-preview`、`/api/install/repair-commit`。沿用同源/CSRF/IP Host、64 KiB 请求预算、单维护任务 owner 与退出等待；浏览器断开不停止已接管的持久化工作。恢复入口只读取原计划并核对当前合法状态，返回实际观察到的程序发布状态，不把原计划当作新操作或系统 ready。

只有准确确认 `repair-same-payload-program-only` 并声明外部 ZXS/Samba/主机管理写者已停，才开始读取或修改程序路径。无认证 root 控制台仍只能临时用于可信隔离网络。声明和建议锁不是对任意 root/旧程序的强制互斥，也不是网络访问认证。

## 原文件准入与计划

目的地仅 `/usr/local/bin/zxs`。计划固定主机根/程序父身份、完整原安装基线摘要、同一 payload 和原文件快照。请求摘要必须是 64 位小写十六进制；恢复不接受浏览器传回的原文件、替代程序或任意路径。

原程序可以缺失，或为 root 所有、普通单链接、0 至 256 MiB 的文件。允许字节损坏、截断或失去执行位；拒绝符号链接、多链接、目录、设备、特殊权限以及组/其他可写文件。不因“修复”放宽可疑路径准入。原快照保存 device/inode、UID/GID、mode、长度和完整 SHA-256，操作阶段重新读取核对。

若当前已是准确内置字节与 0555，不创建新的修复，提示普通首装核验或读取既有原修复请求。原 mode 为其它安全模式时可在保留原文件后将新 inode 恢复为 0555。

## 备份、交换与持久结果

1. 在 `.zxs-helper-program-repair-<摘要>/plan.json` 持久化准确原计划。
2. 若原文件存在，将完整原字节独立复制到 `.zxs-helper-program-original-<摘要>/original.bin`，0600，完成文件/目录同步。不是硬链接、reflink 或原路径之外的引用；不同既有备份拒绝覆盖。
3. 在程序父目录准备 `.zxs-helper-repair-<摘要>.program`，只允许准确 replacement 字节、0555、普通单链接。完整同步后再次核对原文件、基线与父目录。
4. 原文件存在时，使用同目录 `RENAME_EXCHANGE` 原子交换，原 inode 留在确定 staging 名下，原 mode/GID 和字节不改；原文件缺失时使用 `RENAME_NOREPLACE` 发布。没有 unlink、删除旧程序、自动回滚或服务调用。
5. 实际读回新目标和原 inode，确认合法组合，同步相关文件及程序父目录；最后在 `.zxs-helper-program-repaired-<摘要>/receipt.json` 保存准确新 executable inode 与明确未启动/未就绪结果，再核当前目标。

**不是跨 `/zxs` 与 `/usr/local/bin` 的原子事务。** 程序可能已替换而完成回执尚未写成。错误保留原摘要、全部路径和底层原因；rename 的 `Unknown`、已发布父目录同步失败的 `PublishedDurabilityUnknown` 不被抹成“没写入”。不能因为 HTTP 失败就换摘要或删 staging。

## 原请求冷续作

只接纳下列准确组合：

| 原计划 | 当前固定目标 | 当前确定 staging | 允许动作 |
| --- | --- | --- | --- |
| 原文件存在 | 原快照 inode/属性/摘要准确 | 无，或准确 replacement | 保证独立备份后准备/执行交换 |
| 原文件存在 | 准确 replacement | 原快照 inode/属性/摘要准确 | 不再交换，重建耐久证据并完成原回执 |
| 原文件缺失 | 缺失 | 无，或准确 replacement | no-clobber 发布 |
| 原文件缺失 | 准确 replacement | 无 | 不再发布，完成原回执 |

不同正文、丢失原 inode、双 replacement、原文件计划却目标缺失或任意未知组合，均拒绝保留。独立备份不存在时只可从仍持有原快照身份和完整摘要的原 inode 重新取得准确字节；不能从新程序或未知备份猜测原文件。

已有完整完成回执时，先要求当前 executable inode 与原回执准确一致，缺失或替换不补造。已开始却缺 receipt 的完成 bundle 保留拒绝，不承诺自动修复损坏控制记录。原计划目录缺失/不完整也不重造来源。没有 TTL、旧备份清理、强制覆盖或多版本迁移旁路。

成功仅为 `same_payload_program_repaired_not_started`。`data_or_configuration_modified=false`、`service_started=false`、`complete_installation_ready=false`；不打开数据、数据库、标签、凭据或 `/etc` 配置，不执行新旧 ZXS，不 reload/start/enable，不更改网络、账户或 helper 自启。

## 预算与仍未完成

载荷上限 256 MiB，原程序读取上限 256 MiB，计划/回执各 16 KiB。文件比较和复制为固定阶段数的 O(程序字节数)，不扫描池/历史；暂时读取两个程序的动态字节最多约 512 MiB，另有已嵌入载荷及序列化/系统缓冲。每次有原文件的修复保留一个独立备份和原 inode，占用空间不能假称回收；空间不足明确失败。未削减既有 bundle/rename/fsync 屏障，不宣称性能提升。

尚未覆盖：缺安装基线的任意历史安装、不同版本/源码树的升级降级、异常 owner/link/mount 修复、旧备份清理、VFS/依赖/许可材料的完整内置安装包、完整一条龙系统与 SMB 配置及实际发行构建。后续不得把同载荷程序修复扩成数据/配置重置。
