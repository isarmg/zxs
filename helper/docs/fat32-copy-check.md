# FAT32 镜像检查：只让检查器看到副本

`helper check-fat32-copy IMAGE --sha256 HEX` 是一次本机只读请求。仅受理显式绝对路径、普通单链接文件、完整 SHA-256 和不超过 128 MiB 的 FAT32 单卷；不是整盘分区表。原件按普通文件比较的本地文件系统、NOATIME、原实体和完整 EOF 边界读取。先复制到一个新的、禁止执行的 memfd，封死写入、增长、截短与改封印，再逐字节重读此副本核外部摘要。从封存副本验证扇区、簇、FAT、根簇及总长度；仅有 “FAT32” 字样不算格式证据。复制时原件被改或摘要不合不启动检查。它不判断原文件是否曾挂载、是否最新或是否来自某块物理盘。

## 固定受信发行配置

目标是 Debian 13 amd64。正常发行包自行携带固定启动器、上游检查器及私有运行库；不要求用户手配摘要或安装 dosfstools，不使用同名宿主命令替补。构建/发行者须先独立核 Debian 的发行签名、实际程序、配套库、补丁/许可证与完整对应源码，并对最终发行物签名；见[工具包配方](../packaging/debian13-amd64/README.md)。本次没有生成可安装物或签名。

固定根是 `/usr/lib/local-ops-helper/disk-tools-v1`。其中 `fat32-check-v1` 首行为 `debian13-amd64/private-pack-1/bubblewrap-0.12/dosfstools-4.2`；接下来八行必须依次给出 bwrap、fsck.fat、ld-linux-x86-64、libc、libcap、libselinux、libpcre2、libgcc 的最终 SHA-256。每段实际路径及叶均须受保护、root 所有、组/其他人不可写，拒绝符号链接、漏件/漏项、错架构和散列不符。启动器用已核的原文件描述符 exec；只有 fsck.fat、装载器及 libc 会进入检查沙箱。运行中的发行目录、宿主内核和启动环境仍须可信。此处并不动态验证发布签名，实际部署须先核收件来源。读取公开安装文件可能更新它们的 atime。

## 检查权限与输出

只在独立、单线程、无额外组或 capabilities 的非 root CLI 中执行；不调用 shell，不接受其他参数。先设置 no-new-privileges，让未知继承描述符在 exec 时关闭。有界传入的只有已核发行组件、封存镜像和专用状态管道。子进程的 stdin 是 EOF 管道，不借用终端。检查器的独立命名空间只读根只有固定程序、加载器、libc 及 `/image`；不见原文件、宿主 `/dev`、`/proc`、其他卷或网络。bubblewrap 从当前偏移 0 把密封镜像复制进私有只读挂载，因此至多有**两份 128 MiB 副本**，不宣称零拷贝。每次启动在已核 ELF 执行之前即建独立会话和进程组，隔离设置前后均不脱组；最终工具的固定 seccomp 规则禁止 fork、改进程组及切换命名空间。超时、超额、INT/TERM/HUP/QUIT 等取消及正常结束均先向仍由未收割的原 leader 锁定的一组发终止信号，再收割该组的直接和被领养后代；未取得原 launcher 等待状态时绝不报成功。helper 自身若遭 SIGKILL、宿主崩溃或内核 I/O 卡死，不承诺总能在本进程内确认收尾。使用降权后的固定 `/checker -n /image`；不自动检查或修复原盘。

子进程启动后 30 秒墙钟、12 秒 CPU、512 MiB 进程地址空间、64 KiB 合并标准输出/错误等上限。中途信号、超时、输出不全或没有 launcher 的终场回执不得报健康。返回 0 只代表受支持工具完成且无其他检查消息（无宿主字符转换插件时，仅允许工具使用内置 CP850 表的完整已知启动提示），绝不是硬盘健康或以后仍未改变；1 表示工具给出与原几何一致的完整终场统计，另有消息或问题（工具自身的 1 也可含内部不一致）；即便工具退出 1，只要缺本镜像的完整统计，仍为 2：没有完整检查结果。给出终场解析下来的文件数、使用簇和总簇，不回显原始报告。不会把镜像中的文件名、卷标或终端转义原文写到报告。

实现依据：[bubblewrap 0.12 参数和隔离](https://github.com/containers/bubblewrap/blob/v0.12.0/bwrap.xml)、[dosfstools 4.2 工具源码和退出处理](https://github.com/dosfstools/dosfstools/blob/v4.2/src/fsck.fat.c)、[Linux memfd 封印](https://man7.org/linux/man-pages/man2/memfd_create.2.html)。实际受控 Debian 环境、坏镜像和隔离失败的运行验收尚未做。没有其他格式的执行器、修复入口或 Web 写操作。
