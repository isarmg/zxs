# 外部文件离线导入

面向从池外目录迁入文件的操作者。先停 ZXS，确认登记的目标盘及不重叠的来源。
保留来源和原计划，只用同一份计划重放，避免覆盖和重复导入。

## 从非空盘导入一个文件

停止存储服务，用同一个专用服务账号执行。先确定一个原文件和目标文件名，再从当前存储池中指定一个已加入且初始化完成的 DATA 盘。两个命令都要按当前登记顺序填写完整的 SYSTEM、RECYCLE 和 DATA。不会递归导入目录、自动压平路径、覆盖同名文件或替你改名。

选择一个只供自己使用的目录保存导入单。这个目录应在源目录、存储盘和部署目录之外，并使用一个不存在的新文件名。下面的重定向由 shell 完成，不是 Helper 写入文件；`set -C` 避免覆盖已有文件。

```sh
umask 077
set -C
helper import-one prepare /external documents/report.pdf report.pdf /data-two /system /recycle /data-one /data-two > /private/import-tickets/report.ticket
helper import-one apply /system /recycle /data-one /data-two < /private/import-tickets/report.ticket
```

第一条只读取所选文件并在标准输出写出一行导入单；不会改文件，也不会写入容器。源文件必须允许服务账号以不更新访问时间的方式打开。权限或文件系统不允许时直接退出，不降级读取或改变权限。第二条用原始导入单创建一个容器和首个 `main` 节点，保留源文件和原路径。

目标名称区分大小写，若当时已有同名容器则停止。目标为已登记的盘，不用这一命令加盘。

成功后保留输出的容器和节点编号。若结果不明确，保留原文件和原始导入单，用同一份导入单重试；不要重新生成导入单或换盘、改名重试。已完成时会返回原收据，不重新读取原文件或创建第二个容器。导入单含本机路径，保存在你选的位置；Helper 不留副本。

## 从非空盘批量导入所选文件

仍须在停机时指定外部源目录、一个已登记 DATA 目标和完整有序的角色目录。每个源相对路径都要和一个准确的目标显示名配对，即使它们同名；不遍历未选文件，不自动压平、改名或覆盖。按下例一次选两个文件；空格、通配符等按通常的 shell 规则引用。

```sh
umask 077
set -C
helper import-batch prepare /external /data-two /system /recycle /data-one /data-two -- \
  'documents/report.pdf' '项目报告.pdf' 'Report.txt' 'Report.txt' > /private/import-tickets/selected.plan && \
sync -f /private/import-tickets/selected.plan && \
helper import-batch apply /system /recycle /data-one /data-two < /private/import-tickets/selected.plan
```

`prepare` 先核对全部映射的精确同名冲突，再逐个读取、取证；整份准备成功才向标准输出写二进制计划。临时目录及文件按单文件导入的要求选择，尤其不要把计划放进源目录或存储池。计划中有本机原路径，只使用自己生成且安全保存的原件；摘要能查出截断或误改，不能认证提供者。

内存随票据和名称总量增长；每项沿用单文件工作进程。多个独立观察不是源盘快照，也不预留空间或目标名称。

`apply` 须从保存下来的普通文件读入，不能管道直连。它在第一次写入前读完并核对整份计划；在同一个离线排他锁内，按原顺序用**各自原始的单文件导入单**处理，每行输出序号、容器和节点编号。

整批不是一个事务：中断、目标被占、空间不足、源发生变化或回执输出失败时停在当项，较早项目可能已经完成。保留原文件和原计划，只重放**整份同一计划**；此前确认的项返回历史收据，无需再读其源文件，也不会恢复已回收的容器。不要重新准备或编辑计划来续作。命令行里的映射对受操作系统参数长度限制；

超长时可用下文的 `--map0`，或将互不重叠的选择分成多份计划；该命令本身不接管或登记来源磁盘。

## 从整个非空源目录导入文件

`import-batch prepare-tree` 清点所选外部目录内的**全部普通文件**。合法、无冲突的顶层名保持不变；有子目录、名称无法原样保存或目标重名时，明确给出该源相对路径及想使用的目标名。少配、错配、重复或仍有同名时整份拒绝，不自动展平或跳过文件。

先停业务、保持源目录不变，用 `preflight-import --check-target` 查看需要映射的项目；纯顶层、无冲突时省略 `--` 及其后参数。

```sh
umask 077
set -C
helper import-batch prepare-tree /external /data-two /system /recycle /data-one /data-two -- \
  '甲/报告.pdf' '甲组报告.pdf' '乙/报告.pdf' '乙组报告.pdf' > /private/import-tickets/all.plan && \
sync -f /private/import-tickets/all.plan && \
helper import-batch show < /private/import-tickets/all.plan
```

核对上面显示的**源目录、目标 DATA、逐项原路径、名称和字节数**后，执行 `helper import-batch apply /system /recycle /data-one /data-two < /private/import-tickets/all.plan`。

以后只重放这一份，不重新清点来补跑。两次清点在同一个固定原目录上比较全部原名和元数据，每个票据的源文件也要与清点时的原件一致；不是原子快照。准备完成后新增的文件不在旧计划里。目录（包括空目录）、权限和硬链接关系不在目标重建，原件保留；多硬链接按名称各建一个文件容器。

没有普通文件时不生成计划。

大量自定义名可用 `--map0` 代替命令行的全部映射对：在标准输入依次给原始 `相对路径 NUL 目标名 NUL`，最后另加一个 NUL 并结束。这不会按制表符、换行或 UTF-8 切分源文件名；目标名仍需合法。

例如先在私有目录用 `printf '%s\0' '甲/报告.pdf' '甲组报告.pdf' '乙/报告.pdf' '乙组报告.pdf' '' > /private/import-tickets/names.map0`，再以 `… --map0 < /private/import-tickets/names.map0 > /private/import-tickets/all.plan` 准备。

明确选择的 `prepare` 也接受此映射方式。

来源必须在可证明的本地 ext2/3/4、XFS、Btrfs 或 F2FS 挂载上；所选目录可以是其合格子目录。子挂载、符号链接、特殊项、不可读目录或不允许 `O_NOATIME` 的原件都不会被悄悄略过；遇到这些情况，不会生成整树计划。

目标是**已登记且初始化完毕的 DATA**，不是在非空盘上就地格式化、加盘或擦除。

**想让原盘也承担 DATA 角色时：**如果原文件本来都在 `/mnt/archive/originals`，可以在同一已挂载文件系统选择一个符合角色和权限要求的**空兄弟目录**，如 `/mnt/archive/zxs-data`。该目录须归原服务账号所有、权限 0700；其文件系统不能已供另一 DATA 或 RECYCLE 使用（SYSTEM 可以共享）。需要为导入后的完整文件和元数据留足空间；另一块目标盘也是选择。先停机，用 [`setup-headless add-data`](headless-offline.md#离线加入空数据盘) 将**空目录**加到原有清单中；已有池已初始化时，在停机状态下用 [`setup-headless init-data`](headless-offline.md#离线初始化一块已登记的空-data) 初始化新登记的目标盘，无需为它重启服务。此时用 `prepare-tree /mnt/archive/originals /mnt/archive/zxs-data …`，角色参数使用**加入后的完整顺序**，包括新盘。已有且初始化过的目标 DATA 直接从准备计划开始。如果源选的是整个挂载根，其内的 DATA 子目录不是独立目标：不能一边扫描全盘一边默默忽略它，应选另一个不重叠目标。不会替你移动、删除原文件或清空盘。
