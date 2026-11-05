# SYSTEM 目录身份核验：2026-11-05 验证

范围：`c7f4f0f2c` 的 `PrivateDirectory::verify_identity` 修正及随后仅 rustfmt
换行的测试文件。不是完整 SYSTEM/SQLite 持有目录接线，也不启用交接中的
trusted-data-recovery 功能。剩余设计和准入门槛见
[ADR 0014](../adr/0014-system-directory-ownership.md)。

## 已执行

使用仓库固定 Rust/Cargo 1.99.0，Linux x86_64：

- `cargo test -p platform-io --lib fs_safety::tests::directory_identity -- --nocapture`：
  新增两项测试通过。覆盖指回同一 inode 的祖先链接，以及缺失、悬空、普通目录替换；
  拒绝后原 held 目录仍可读取/写入，不修复替代路径。
- `cargo test -p platform-io --lib -- --nocapture`：90 通过、2 失败、3 ignored。
  两项失败均为已有测试创建临时 Unix socket 时 `UnixListener::bind` 返回
  `EPERM`，不是身份断言失败：
  `fs_safety::linux::tests::rejects_root_and_intermediate_links_and_all_non_regular_files`
  和 `fs_safety::unix::tests::inventory_rejects_linked_root_special_files_and_excessive_depth`。
  对第一项使用已编译 libtest 在放宽执行沙箱后重试，仍在同一 bind 返回 EPERM。
  不将这两项报告为通过，不删除或放宽测试断言。
- `cargo test -p platform-io --test sqlite_foundation`：2 通过。
- 对修改的 Rust 文件执行 `rustfmt --check --edition 2024`：通过。
- 修改范围 `git diff --check`：通过；涉及三个文档的 24 个相对链接目标存在。
  此目标存在性检查没有代替完整 contract-check 的锚点验证。

另以 `cc` 编译仓库原始 SQLite amalgamation 的独立 C 诊断程序，使用 Unix VFS
打开 `/proc/self/fd/<directory-fd>/index.sqlite`，断言 `sqlite3_db_filename` 返回
实际 `/tmp/.../index.sqlite` 而非 fd 别名。SQLite 3.51.3 下断言通过。这只确认
默认 Unix VFS 会还原可替换的命名路径，不是 SQLx 完整特性矩阵或安全 VFS 验收。

## 未执行及仍需完成

- 三项 ignored 平台故障注入驱动、完整工作区检查与物理断电测试未在本记录内执行。
- 新增身份用例没有提交 syscall 零候选探测证据；通过行为断言不替代 ADR 的 syscall
  正校准、并发交错及后续 SQLite VFS 验收。
- writer.lock、SYSTEM 配置/工作区/lifecycle、SQLite main/WAL/SHM、snapshot 和
  quarantine 的统一 owner 迁移仍未完成。此提交只修正显式身份核验中的祖先跟随。

复杂度变化：命名核验由整路径 metadata 改为深度 d 的 O(d) 次目录打开，
额外同时存活句柄 O(1)，未增加 fsync；没有性能提升声明。
