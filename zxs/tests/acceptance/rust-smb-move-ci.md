# GitHub Actions 同一虚拟机 MOVE 验收

手动任务固定验证两份独立的一次性 Linux 池：[普通跨卷客户端用例](rust-smb-client.md#普通数据卷到普通数据卷)的 `stock` 和 `original`。同一作业以同一已审主线修订构造独立的 Helper 和 `zxs`，比较原 ELF 整文件的独立批准值后才制作种子。任务不重用旧制品，不改变原自动 CI。

## 运行前

- 分别批准本次精确主线修订、按同一构建配方测得的 `zxs` 和 Helper 整文件 SHA-256，以及 `guest_smb_move_inputs.json` 的 SHA-256。缺少材料或摘要不符，任务停止。
- 离线批准记录为 `SMB-MOVE-INPUTS-1`：`profile`、固定 Ubuntu `image_sha256`、本目录从签名索引核对的包表的 `packages_sha256`、Ubuntu `smbclient` 整 ELF 的 `client_elf_sha256` 和 `providers`。`providers` 只含 `gcrypt`、`gpgerror`，每项为实际 Ubuntu 版本原成员 `member`（版本化文件名）及其独立 `sha256`。包归档 SHA 与其中原文件 SHA 是不同的证据。构建运行中的现算值不能代替独立批准。
- 确认只在两份同规格、无外部网卡的全新来宾中，分别挂载四份新虚拟 ext4，准备原 `/zxs` 布局及本次固定客户端、离线提供器文件；只给本次凭据进程现定 128 MiB 的 MEMLOCK 软／硬限额；只配置本次来宾 veth、客户端短期网络命名空间、测试链路 IPv6 与该命名空间的 TCP 445 过滤。未确认这些操作前只做源码和构建核对。

## 实际正向证据

1. 复用[配对构建](../../../.github/workflows/linux-paired-build.yml)的固定工具链、浏览器嵌入校验和独立 `private-fresh-install` Helper。配对测量仅保留独立 `measurement.json`，使用同一构建和精确的 v19 目录；其摘要须单独批准后才能交给来宾。源自同一修订的历史只读观察程序以前置 `cargo test --no-run` 单独产生，不进入正式 ELF。
2. 复用[来宾挂载预检](../../../.github/workflows/linux-guest-preflight.yml)和[终端观察](../../../.github/workflows/linux-guest-terminal-preflight.yml)的签名 Ubuntu 镜像、无外部网卡 QEMU TCG 与只读种子。分别新建 S/F1/F2/R 四份整盘 ext4，验证虚拟序列号、设备、大小、RW 保护挂载和不同 UUID。之前的只读 A/B 与两份 Helper 测试盘不同于这四份盘。
3. 在原真实前台 PTY 完成 INIT、SETUP 和正常 `zxs` 启动，核 exact preview/原回执、关闭的终端审计、候选凭据进程限额及原产品内存锁检查。实际确认两个降权同 ELF 子进程共用放行；独立 Web 登录和签名 SMB 用客户端验证监听真能用。口令只在来宾中临时生成，进入原隐藏终端和继承的密封只读 FD。Runner 没有外部桥接、代理或共享主机盘。Ubuntu 官方 Samba 仅用其独立客户端；只传原执行文件和依赖文件，未运行包安装脚本或 Samba 服务。
4. 真实 SMB 写入 A、B 后以原 Web 手动表单分别提交，保留两个 Node 和 SAVE。`stock` 用签名 Samba rename 验证现行 B；`original` 验证同一原 OPEN 仍读 B、签名完成、重新打开读 C。对照原 SYSTEM 稳定的 FileId、唯一 kind-12/direction=3、原放置代和准确工作头。通过实际持有的对应 F1/F2 分别读出两个历史全文，核对原源名称清退，再真正排空和回收服务。丢答、目标磁盘失联另作验收。

两份真实用例和来宾均退出后才写出 `result.json`：它绑定本次请求、两个独立的 PoolId/FileId，以及九类已核证据摘要。失败只报读数阶段；终端应答、账号内容和来宾原始日志不上传。已发布的配对编译、挂载、PTY 各自只证明那一项通过。完整 MOVE 必须看此任务的实际两案结果。
