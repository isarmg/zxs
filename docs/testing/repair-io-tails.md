# 修复与永久损失分类的 I/O 尾部回归

普通测试覆盖撤销后拒绝修复、冻结候选/日志不变、隔离正文与元数据 inode 保留、重建校验清单字节一致，以及 MISSING/CORRUPT/设备不可用的区别：

```sh
cargo test --locked --offline -p storage-engine --lib repair::tests
cargo test --locked --offline -p storage-engine --lib degraded_recovery::tests
```

以下 Linux GitHub runner 命令使用测试专用 interposer。在真正的修复/恢复调用方进入 read、close 或 fsync 时阻塞，并在测试释放后继续调用内核，或注入 fsync EIO。它不属于生产代码，也不模拟物理设备卡死、掉电或真实盘删除。

```sh
cc -shared -fPIC -Wall -Wextra -Werror \
  -o /tmp/repair_io_tails.so test/engine/unit/repair/io_tails.c -ldl
LD_PRELOAD=/tmp/repair_io_tails.so cargo test --locked --offline \
  -p storage-engine --lib repair::tests::io_tail_tests -- --ignored --test-threads=1
LD_PRELOAD=/tmp/repair_io_tails.so cargo test --locked --offline \
  -p storage-engine --lib degraded_recovery::tests::classification_ \
  -- --ignored --test-threads=1
```

阻塞用例必须实际观察到对应 syscall，不能因 interposer 未加载或未命中而静默通过。每个临时夹具单独启用门控，超时只用于测试失败退出。覆盖正文与元数据隔离的源目录优先、目标目录后续同步；两个同步阶段的取消和 EIO；保留原始 EIO 来源；源读取/关闭与分类探测关闭期间的 descriptor、计数和 writer flock；EIO 后重试以及元数据目标碰撞不覆盖两边证据。

分类读失败分别注入 checksum 与正文的真实 read EIO，必须返回带原始来源的 RECOVERY_REQUIRED，不能因后续 presence open 成功而记作 CORRUPT。关闭回归跳过布局查找的 O_PATH 与普通文件存在探测，精确阻塞分类器／来源校验实际持有文件的 close。

这些源码测试不代表已执行通过，不授权生产离线、卸载或删除。运行结果必须绑定实际提交和 runner 记录。
