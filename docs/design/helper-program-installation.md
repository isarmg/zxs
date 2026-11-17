# 辅助工具：内置载荷与程序首装

[辅助工具](../components/helper.md) · [独立控制台](helper-console.md) · [任务清单](../../TASKS.md)

本片提供内置匹配 ZXS 可执行文件，以及**固定路径、只增不换**的普通程序安装源码；后继已有单独确认的[同载荷修复](helper-same-payload-program-repair.md)，不是任意版本覆盖。不是系统初始化或一条龙依赖／systemd／SMB 配置完成。按最新开发指示，没有构建辅助工具／安装载荷、运行测试或 CI，也没有操作真实系统。

## 不递归的可验证载荷

后继有独立[内置许可/版权材料切片](helper-embedded-legal-materials.md)：从准确 source tree 准备中央文本及逐文件清单，与同一 ZXS 载荷绑定，作为 helper 内部资源提供 CLI/Web 导出。不替代源码/VFS/OS 依赖安装包或独立许可复核；不新增顶层 Release 资产。

`release-native.py` 先构建 ZXS，再将这份准确输出交给 helper 的构建脚本；helper 不被放入自身载荷，不嵌入包含 helper 的完整归档。构建脚本只读取、不执行载荷，要求：

- 与 helper 相同的 Cargo 版本，明确当前 Git commit/tree；
- 当前唯一登记原生目标 `x86_64-unknown-linux-gnu`；正式构建入口仍要求 Debian 13 amd64；
- 单个普通 ELF64、little-endian、x86_64 可执行文件，大小 64 字节至 256 MiB；
- SHA-256 与先完成的 ZXS 输出一致，复制到 `OUT_DIR` 后将快照字节编入 helper。

正式构建收据增加 `native-helper-embedded-zxs=true`、`native-helper-payload-sha256`，后者必须等于独立 ZXS 资产的摘要。发布验证器拒绝缺失或不一致的关系。这是源码／构建输出绑定，不是独立数字签名、编译器可信性或完整可复现构建证明。

普通开发构建没有载荷，状态明确返回 `embedded:null`，安装拒绝。不存在运行时下载、读取身旁任意 `zxs`、接受浏览器上传程序或依赖 PATH 中另一个可执行文件的回退。构建环境变量仅供受控构建入口使用，不是运行时安装设置。

## 入口与明确确认

```sh
zxs-helper install payload
sudo zxs-helper install preview
sudo zxs-helper install commit --request-digest REVIEWED_DIGEST --confirm-program-only
```

独立 Web 增加 `GET /api/install/payload`、`POST /api/install/preview` 和 `POST /api/install/commit`。沿用无登录、同源／CSRF、明确确认和单维护任务 owner；确认内容为 `install-program-only`，绑定刚预览的摘要。控制台醒目提示任何网络可达者都可能安装程序，不能把 CSRF 当认证。

安装位置固定为 **`/usr/local/bin/zxs`**，本片没有任意浏览器写路径参数。父目录必须已存在，由当前 root 所有且不可被组／其它用户写入。预览只读取，输出内置版本、来源、目标、父目录 device/inode 字符串及其绑定摘要；执行重新打开、独占锁定父目录、复核摘要，不相信浏览器提交的路径或程序字节。

## 发布和重试边界

- 目标不存在时，将准确内置字节完整写入新 inode，设为 `0555`，同步文件后以 `NOREPLACE` 原子发布并同步父目录。
- 目标已经是同一安全 owner、单链接、正常可读可执行且正文逐字节匹配的文件时，仅重新核验并同步持久化，结果为 `matching_program_durably_verified`，不声称一定是本次创建。
- 目标包含任何不同字节、错误大小、不安全权限、符号链接、硬链接、目录或设备时，明确拒绝，不覆盖、不删除、不降级成“修复成功”。
- 稳定父目录句柄与非阻塞排他锁贯穿检查和发布；其它 helper 操作不排队覆盖。既有原子发布层保留 rename 结果不确定或目录同步失败的阶段、具体 I/O 来源、目标与 staging 身份；不能把异常当作“肯定未写入”。
- 浏览器断连不释放已经接管的 worker 或许可，退出等待它真正结束。中断后可用相同内置载荷重新预览／提交：仅在尚无已安装基线时允许首装缺失目标；已有原基线时必须核对在场的准确程序，不在普通安装里重建遗失的程序。绝不因此覆盖不同材料。若诊断保留了不确定 staging，应保留而非自动清扫；本片未提供任意残留文件清理功能。

安装、同载荷修复和部署前核验通过持有的 /usr、local、bin 逐段确认正式程序目标；VFS 安装及 Samba 激活对 lib、架构、samba、vfs 逐段绑定，到模块哈希读完后还核对原名与原实体。即便整个原 bin／vfs 目录和原文件被带到同路径的新上级，或上级变成其他用户可写，也不承认原绑定；不据此声称所有外部 root 写者已停。

可执行文件和同源码 VFS 的核验按固定 32 KiB 读缓冲逐字节比较，保留首次确认的文件描述符、身份与内容变更戳至最后同步及原收据读回；配置／启动准备也握住同一已准入的程序。无法以相同字节的另一个 inode 完成已在进行的操作。VFS 只有正式发布的 intent 和完成目录才是历史完成；即使 pending 完成目录已有完整正文，也不当作已完成、不能给缺失模块补写。单独的同载荷修复必须由已发表的原程序基线授权，内容已写齐但仍在 pending 名下的基线不能授权修复。当前单一格式不存跨调用的执行文件 inode；重新打开只重新核验当前同载荷和已发表原记录，不宣称历史身份从未变化。

成功收据仅说明准确程序字节已发布／核验，明确 `system_initialized=false`、`service_configured=false`、`service_started=false`。程序没有被执行，没有新建数据库、标签或盘绑定，也没有改变账户、密码、systemd、Samba、网络或自启配置。

普通安装仍不替换不同既有文件或修改运行池。后继首装先锁定既有 `/zxs` 再锁程序父目录，完成后持久保存不可变的同载荷安装基线。单独的修复仅接受该准确基线，先备份原字节并保留交换原 inode，不是升级/降级，也不能把当前 daemon 建议锁当作所有外部写者已停。程序系统依赖检查／安装、VFS 和完整许可材料内置以及总体初始化交接仍是后续工作。

公开发行资产四类收敛仍未完成；本片只改变 native 构建顺序和载荷绑定，不新增第五类资产，也不宣称已把现有七资产脚本收敛为四类。安装载荷未实际生成，所有新增测试只是源码。

后继[部署配置准备](helper-deployment-preparation.md)只生成并原子保存未激活的 systemd/Samba 文件，不改变本程序首装的 `service_configured=false` 或将准备包当成运行条件已满足。
