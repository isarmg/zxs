# 一期 SMB 普通新文件回归（2026-11-06）

基线为 `c17e3ad90042ef83759c24da635449f24e14b35e`，源码树为
`c9bbfcffc368c63bf8e8c4a861716be6560fc844`。本增量只增加三项回归及私有模块接线，不改变产品行为。

## 场景与判据

测试位于 `test/server/unit/smb_rpc/phase1_new_file_tests.rs`，通过已有
`default_share_tests` 私有辅助函数建立真实 Engine 和 Unix socket RPC，
使用 `Config::development([p])` 的单一级盘，没有二、三级盘依赖。

1. 源文件保存两个不同正文版本，再设置源专属标签、文件及版本备注。
   客户端通过 `Read` RPC 取当前正文，在另一目录独占新建目标，依次准入写入、
   Dirty、Flush、Close。目标 entry/file/version 身份独立，只有一个正文版本、
   一个节点、一个分支，没有 source_version_ids、父节点、标签、合并和源备注。
   冷开后再次核对源的两个正文、标签、备注与目标正文。
2. 新空文件不发送 PrepareWrite、Dirty 或 FD 写入，分别直接 Close 及
   Flush 后 Close。冷开后每个文件恰好一个独立的零字节版本；重复保存边界
   不增加多余版本。
3. 对已保存的 `Résumé.txt` 以同名、大小写等价名和 NFD 等价名执行
   create+exclusive+truncate，必须返回 `NameConflict`、不返回 FD。
   现有 entry/file/version 与正文、spool 目录条目保持不变，没有残留 writer；
   原 RPC 连接随后仍能成功新建另一文件。冷开再次核对原文件只有一个版本。

正常目录提交可以引用此前目录提交；测试不禁止这个合法祖先关系。测试不探测
客户端的“复制”意图，也不使用复制来源、复制历史或旧数据迁移接口。

## 实际验证与资源边界

已执行 Rust 1.99.0 `rustfmt --edition 2024 --check`、`git diff --check`，
并检查私有子模块接线能够解析至新增文件。完成 typed API 与恢复生命周期的
静态审查。这些结果不等同于 Rust 类型检查或运行成功。

原生编译及三项运行结果**待一期集成统一执行**。三项测试依赖 Server libtest 的私有传输与工作区夹具，不能直接外链已有生产 rlib。因此其它公开 HTTP 测试或生产库编译通过，均不能代替本目标的正式编译与运行。

在完成源码合并并确认统一构建资源后运行：

```sh
cargo test --offline --locked -p storage-server --lib phase1_primary_ -- --test-threads=1
```

命令使用包的默认 `core` 特性，以及现有 server dev-dependency 启用的
Engine `fault-injection` 特性。记录实际合并提交、源码树、工具链、profile、
RUSTFLAGS、target 目录和依赖来源。此处未验证真实 Samba/VFS 客户端、
网络 SMB 端到端、多级备份或跨盘完整历史。
