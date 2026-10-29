# Existing immutable history-object barriers

Normal `workspace` CI invokes `.github/scripts/check-history-object-io.py`. It builds the engine libtest with Cargo JSON output, selects its exact artifact (no stale binary glob), then calls `test/engine/unit/history_package/immutable_io/run.sh`. The shell preloads only each selected test child; no production process or global environment is changed.

The Rust test first installs an empty `state-page` on three configured members using the real immutable writer, then writes exact target paths and enables an observation marker. The C interposer observes libc `fsync` on those exact regular files. A successful target file barrier arms only that member's exact parent-directory barrier; unrelated kind/shard directory syncs are not counted as this object's durability. The selected single-thread test performs no unrelated object stores in this armed interval.

Seven children cover success, and one-shot file/directory EIO on each of the three members. Logs distinguish attempted, successful and injected-failure calls. Exact log assertions prove the interposer was actually reached, retain the first error's EIO source, and require full barriers both when retrying the same plan and when preparing again. The successful child also stores an undeduplicated two-reference control to establish the difference from the unique plan.

This checks the **existing-target** branch: one successful unique store on three members must emit three successful file barriers and three successful directory barriers. It is not a count of all process syscalls, raw rustix calls, initial directory creation, or physical power-loss behavior. No callback count substitutes for these observed libc calls. Fresh-object atomic creation retains the separate platform `atomic-io.sh` suite.

Run after building via the Python CI entry point, or pass an already-built engine libtest binary to `test/engine/unit/history_package/immutable_io/run.sh`. Missing observation, unexpected events, real I/O failure or unpreserved error sources fail the test. Merely compiling these files is not an executed durability result.

After every assertion the Rust child writes its case-specific completion marker. The shell requires its exact stage/member identity in the fresh case directory; libtest exit 0 with zero matching tests cannot pass. The CI command-suite tests execute zero-test and silent-success fake binaries and require the specific missing-completion diagnostic after proving the child ran.

## 元数据对象目录创建与重试

同一 CI 入口复用上述 Cargo 精确 libtest 产物，再执行
`test/engine/unit/disk/owned_directory_io/run.sh`。七个根测试通过十个隔离子进程覆盖：
child/parent 的单次真实 libc `fsync` EIO、同资源重试、新进程重开、目录身份替换、
四线程并发、证明逐出及 `Engine::open` 的真实 `CompletePending` 恢复。
首次调用确认 mkdir 已生效而对象文件尚未安装，并沿 source 链找到原始 EIO；
重试成功前必须观察到欠缺的父屏障，稳定的后续同对象写不重复祖先创建屏障。
真实恢复还核对原 operation、唯一仓库与再次重开的相同永久收据。
runner 检查精确测试存在、单项通过数、完成标记，并在失败时也输出 syscall 事件。

复用证明只属于 journal 类型化对象地址指定的固定 `metadata/objects/kind/shard` 树，
不用于可能改名的容器目录。证明持有并核对 child/parent FD，两个同步均成功后才缓存；
资源重开、身份替换和逐出后重新建立。每个 DiskRoot 最多保留 128 项、256 个目录 FD，
随成员数累积；创建准入的 mutex、冷路径同步及逐出后重建均有成本，未由此宣称性能提升。
普通目录写入与根在外部父目录中的创建持久性不在这一修复范围。

这套注入只返回真实 I/O 错误并观察调用顺序，不模拟存储设备掉电；进程重开成功
不能作为物理掉电持久性验证。测试代码和接线存在也不能代替固定提交的 GitHub 执行结果。
