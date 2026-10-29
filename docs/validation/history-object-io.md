# Existing immutable history-object barriers

Normal `workspace` CI invokes `.github/scripts/check-history-object-io.py`. It builds the engine libtest with Cargo JSON output, selects its exact artifact (no stale binary glob), then calls `test/engine/unit/history_package/immutable_io/run.sh`. The shell preloads only each selected test child; no production process or global environment is changed.

The Rust test first installs an empty `state-page` on three configured members using the real immutable writer, then writes exact target paths and enables an observation marker. The C interposer observes libc `fsync` on those exact regular files. A successful target file barrier arms only that member's exact parent-directory barrier; unrelated kind/shard directory syncs are not counted as this object's durability. The selected single-thread test performs no unrelated object stores in this armed interval.

Seven children cover success, and one-shot file/directory EIO on each of the three members. Logs distinguish attempted, successful and injected-failure calls. Exact log assertions prove the interposer was actually reached, retain the first error's EIO source, and require full barriers both when retrying the same plan and when preparing again. The successful child also stores an undeduplicated two-reference control to establish the difference from the unique plan.

This checks the **existing-target** branch: one successful unique store on three members must emit three successful file barriers and three successful directory barriers. It is not a count of all process syscalls, raw rustix calls, initial directory creation, or physical power-loss behavior. No callback count substitutes for these observed libc calls. Fresh-object atomic creation retains the separate platform `atomic-io.sh` suite.

Run after building via the Python CI entry point, or pass an already-built engine libtest binary to `test/engine/unit/history_package/immutable_io/run.sh`. Missing observation, unexpected events, real I/O failure or unpreserved error sources fail the test. Merely compiling these files is not an executed durability result.

After every assertion the Rust child writes its case-specific completion marker. The shell requires its exact stage/member identity in the fresh case directory; libtest exit 0 with zero matching tests cannot pass. The CI command-suite tests execute zero-test and silent-success fake binaries and require the specific missing-completion diagnostic after proving the child ran.

## 元数据对象目录创建与重试

同一 CI 入口复用上述 Cargo 精确 libtest 产物，再执行
`test/engine/unit/disk/owned_directory_io/run.sh`。九个根测试通过十三个隔离子进程覆盖：
child/parent 的单次真实 libc `fsync` EIO、同资源重试、新进程重开、目录身份替换、
四个独立安装并发、有界批次分段及 `Engine::open` 的真实 `CompletePending` 恢复。
首次调用确认 mkdir 已生效而对象文件尚未安装，并沿 source 链找到原始 EIO；
重试成功返回前必须观察到欠缺的父屏障。已存在的对象仍可先完成文件与 shard 屏障，
再由本批次完成祖先屏障；这些义务必须在下一 journal 阶段前结束。
真实恢复还核对原 operation、唯一仓库与再次重开的相同永久收据。
runner 检查精确测试存在、单项通过数、完成标记，并在失败时也输出 syscall 事件。

ABA 用例先安装对象，移出 `publication`，成功同步 `objects` 使删除名称持久化，再把
同一个 inode 移回原名且不补父同步。父子路径和 dev/ino 均保持不变，下一新对象安装
仍须完成本次 `objects` 屏障；指定该同步 EIO 时须保留来源并失败，后续重试再补齐。
当前权限、不可移动协议约定及相同 FD 身份都不能代替本次同步义务。

同步状态只存在于显式 `ObjectInstallBatch`，不保存在 DiskRoot 或跨独立调用复用。
每次写入从持有根重走实际祖先并登记父与 shard 的 FD；同路径身份变更明确拒绝。
批次完成按深度核对当前路径与持有身份，逐个去重同步实际父目录。shard 本身仍由
每个原子对象写入同步。单对象 `store_on` 与修复入口自闭合批次，返回后才允许 journal
追加；`PreparedSnapshot` 两个真实安装入口则按成员在一次调用内合并目录义务，全部
完成后才累加已完成工作指标。失败不靠 Drop 同步，也不把尚欠屏障的对象算完成。

同一显式批次的三个相同对象写只在完成点同步一次公共父目录；随后独立调用必须
重新同步自己的父链。每批最多暂留 128 个目录 FD（另计瞬时 FD 与既有资源），
达到预算先完成当前子批并释放，再为下一对象重新登记整条链，不能继承已同步集合。
批次成本随参与成员和触及目录增长；未做 NOFILE 或物理性能验收，不宣称性能提升。
普通目录写入与根在外部父目录中的创建持久性不在这一修复范围。

这套注入只返回真实 I/O 错误并观察调用顺序，不模拟存储设备掉电；进程重开成功
不能作为物理掉电持久性验证。测试代码和接线存在也不能代替固定提交的 GitHub 执行结果。
