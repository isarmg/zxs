# 24 技术资料与引用范围

[文档目录](README.md) · [上一章：当前支持范围、限制与设计取舍](23-delivery-and-decisions.md) · [下一章：唯一管理员操作手册](25-operation-manual.md)

<a id="s24"></a>

规范所引用的官方资料及其适用边界。

以下资料用于核对 SMB 服务、命名、持久化和数据格式的基础语义。分层副本协议、分支规则、路径预算、默认值及模块划分均是本系统设计，不是官方资料作出的可靠性保证。在线资料会更新，发行时仍须锁定实际版本并测试。

| 编号 | 官方资料 | 本文使用范围 |
| --- | --- | --- |
| [S01] | Git：What is Git? | 逻辑快照的基本概念，不直接套用其物理存储 |
| [S02] | Git：Branches in a Nutshell | 分支和提交引用概念；本系统额外要求全量创建 |
| [S03] | Samba：smbd | SMB 服务进程、共享与客户端接入基础，不证明本系统版本事务 |
| [S04] | Samba：smb.conf | 协议、安全、共享及 VFS 配置依据；业务桥接需另行实现验证 |
| [S10] | Btrfs：btrfs-scrub | 数据校验、修复及 NOCOW / NODATASUM 边界 |
| [S11] | SQLite：Write-Ahead Logging | 本地 WAL 范围，不是跨盘事务 |
| [S12] | Linux man-pages：fsync(2) | 文件与目录项持久化的区别 |
| [S13] | Microsoft：Naming Files, Paths, and Namespaces | 禁止字符、设备名、大小写和末尾限制 |
| [S14] | Unicode UAX #15 | Unicode 规范化 |
| [S15] | Microsoft：Maximum Path Length Limitation | 路径长度及应用启用条件 |
| [S16] | Btrfs：Reflink | 共享 extent 的机制，用于定义独立文件约束 |
| [S17] | SQLite：Online Backup API | 一致数据库快照 |
| [S18] | IETF RFC 9110 | HTTP 范围、实体标识与条件请求 |
| [S19] | systemd.exec | 服务沙箱与挂载命名空间边界 |
| [S20] | Unicode 16.0.0 | 固定数据版本及配套文件 |
| [S21] | Btrfs：btrfs(5) | 只读、日志重放及离线恢复挂载选项 |
| [S22] | BLAKE3 官方实现 | 哈希实现来源，不等于系统安全证明 |
| [S23] | RFC 8785 | JSON 规范表示及大整数边界 |
| [S24] | RFC 8259 | JSON 编码、解析和互操作基础 |
| [S25] | JSON Schema 2020-12 | 契约字段、类型与约束表达，不证明持久化或授权正确 |
| [S26] | OpenAPI 3.1.0 | 核心 HTTP 接口草案的机器可读描述 |

[S01]: https://git-scm.com/book/en/v2/Getting-Started-What-is-Git%3F
[S02]: https://git-scm.com/book/en/v2/Git-Branching-Branches-in-a-Nutshell
[S03]: https://www.samba.org/samba/docs/current/man-html/smbd.8.html
[S04]: https://www.samba.org/samba/docs/current/man-html/smb.conf.5.html
[S10]: https://btrfs.readthedocs.io/en/latest/btrfs-scrub.html
[S11]: https://www.sqlite.org/wal.html
[S12]: https://man7.org/linux/man-pages/man2/fsync.2.html
[S13]: https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file
[S14]: https://www.unicode.org/reports/tr15/
[S15]: https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation
[S16]: https://btrfs.readthedocs.io/en/latest/Reflink.html
[S17]: https://www.sqlite.org/backup.html
[S18]: https://www.rfc-editor.org/rfc/rfc9110.html
[S19]: https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html
[S20]: https://www.unicode.org/versions/Unicode16.0.0/
[S21]: https://btrfs.readthedocs.io/en/latest/btrfs-man5.html
[S22]: https://github.com/BLAKE3-team/BLAKE3
[S23]: https://www.rfc-editor.org/rfc/rfc8785.html
[S24]: https://www.rfc-editor.org/rfc/rfc8259.html
[S25]: https://json-schema.org/draft/2020-12
[S26]: https://spec.openapis.org/oas/v3.1.0.html

---

[文档目录](README.md) · [上一章：当前支持范围、限制与设计取舍](23-delivery-and-decisions.md) · [下一章：唯一管理员操作手册](25-operation-manual.md)
