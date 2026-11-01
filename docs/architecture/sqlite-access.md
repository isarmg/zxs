<a id="sqlite-access-architecture"></a>

# SQLite 访问层架构

维护的 `sqlite-access` 将原 SQLx facade、查询引擎和 SQLite 驱动合为一个 Cargo 包，而非三个改名包装包。原生 `libsqlite3-sys` 继续独立，因为它负责 SQLite C 构建与原生链接契约。

## 从哪一层开始

产品索引应使用 [platform-io 的连接策略](../../src/platform/src/sqlite/mod.rs)，由它配置 WAL、同步级别、外键、忙等待和原生安全限制。不要把下列驱动示例当成产品数据库的初始化或恢复流程；DDL、目录权限、备份与重建仍由引擎负责。

编写驱动或独立工具时，可直接使用 `sqlite_access::{Connection, SqliteConnection}`：

- `SqliteConnectOptions::new().filename(path)` 接收字面文件名，适合已获得 `Path` 的调用者；`.create_if_missing(true)` 才允许创建缺失文件。
- `SqliteConnection::connect("sqlite::memory:")` 或字符串解析适合 SQLite URI。URI 编码和字面路径不是同一种输入，不要先手工百分号编码再交给 `.filename()`。
- `query` 执行语句，`query_scalar` 读取单列，`query_as` 映射到实现 `FromRow` 的类型（如元组）；没有 `query!` / `query_as!` 或 `FromRow` 派生宏。
- 数据值用 `.bind()`，不要拼接进 SQL；动态查询用 `QueryBuilder::push_bind`。占位符不能替代表名或列名，标识符应来自受控的结构。

以下仅创建私有内存库，需要 `sqlite-bundled`，可由任意能驱动 future 的原生执行器调用，不要求 Tokio：

```rust
use sqlite_access::{Connection, SqliteConnection};

async fn sqlite_example() -> Result<(), sqlite_access::Error> {
    let mut connection = SqliteConnection::connect("sqlite::memory:").await?;
    sqlite_access::query("CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT NOT NULL)")
        .execute(&mut connection).await?;

    let mut transaction = connection.begin().await?;
    sqlite_access::query("INSERT INTO notes (title) VALUES (?1)")
        .bind("本地文档")
        .execute(&mut *transaction).await?;
    transaction.commit().await?;

    let title: String = sqlite_access::query_scalar("SELECT title FROM notes WHERE id = ?1")
        .bind(1_i64)
        .fetch_one(&mut connection).await?;
    assert_eq!(title, "本地文档");
    connection.close().await?;
    Ok(())
}
```

`&mut *transaction` 是事务内真实连接的借用。`commit()` / `rollback()` 消耗事务，嵌套 `begin()` 使用 savepoint；未完成事务在 drop 时启动回滚。`fetch_one` 无行时返回 `RowNotFound`，允许不存在时使用 `fetch_optional`。需要确定工作线程与文件句柄已关闭时应显式 `.close().await`，包括产品错误清理路径，不能仅靠 drop 后立即移动数据库文件。完整错误路径可参考上述平台实现。

行按所有权跨线程传递，保留 `Send`；`SqliteRow` 不提供 `Sync`。SQLite 值转换与借用缓存具有内部状态，共享行引用不能用于并发解码。`busy_timeout` 在创建工作线程和打开数据库前检查整毫秒是否超过 `i32::MAX`，越界返回 `InvalidArgument`，不创建文件。启用 `sqlite-preupdate-hook` 时，替换、显式移除和关闭连接均释放其闭包捕获资源。

<a id="module-boundaries"></a>

## 模块边界

- `src/query_engine`：池、事务、语句、类型化值、查询构建器、错误与语句缓存，保留 `forbid(unsafe_code)`。
- `src/sqlite`：SQLite 类型、选项、工作线程连接与原生 FFI，是唯一允许 unsafe 的模块。
- `src/lib.rs`：统一查询 API 与 SQLite 类型。真实消费者需要的通用查询模块继续可用，包括 `common::StatementCache`；实现模块本身私有。
- `test/foundation/sqlite-access/{facade,query_engine,sqlite}`：保留的上游测试，分别归属于原包。已删除数据库后端/宏的归档测试只作来源材料，不代表支持的 API。
- `test/platform/unit/sqlite/driver`：本地编写的直接连接及解析契约，与上游测试分离。

共享 `TransactionConnection` 属于 `transaction::connection`。物理文件仍为 `src/query_engine/pool/maybe.rs`，使用显式模块路径保留不可变来源历史归属；不恢复旧公开模块，也不引入兼容包装。

<a id="feature-policy"></a>

## 特性策略

生产继续选择 `sqlite-bundled`。直接文件名/connect-with、事务和 SQLite 专用字符串解析不需要 Tokio、URL 主机名/IDNA/Unicode 规范化。百分号与表单 codec 保留，因为 SQLite URI 选项及编码文件名需要它们。

- `pool` 显式启用 Tokio 多连接调度、信号量、空闲队列和关闭通知。`platform-io/sqlite-pool` 暴露平台保留的异步多连接/快照池适配器；同步快照和引擎直接工作线程无需该特性。
- `url` 启用通用类型化 `Url` 互操作，包括 `ConnectOptions::from_url`、`to_url_lossy`。字符串 `connect` / `FromStr` 保留原 SQLite 专用解析，不启用通用解析器。
- 事务独立拥有借用或池化连接来源，不依赖 pool 模块；禁用池不会产生第二套事务实现或兼容 facade。

系列门禁执行默认与保留的可选能力，断言最小生产闭包排除池调度和通用 URL 解析，并验证禁用 API 无法编译。这只裁剪所选构建闭包，不减少仓库总活动包数，也不移除保留的源码/许可证来源。

来源目录另将 `sqlite-pool-validation` 记录为非生产、编译用途 profile，以 `platform-io` 为根且仅启用 `sqlite-pool`，对应可选平台测试。原生池通知仍需要 `parking`；它既不是默认产品依赖，也不是可退役的无用包。特性专用元数据与默认 profile 独立解析，闭包记录请求特性集合。

`?NNN` / `$NNN` 参数要求完整 ASCII 十进制及溢出检查，拒绝零、非法后缀和超范围序号；保留前导零、重复/重排参数及缺参为 NULL，并继续检查缺参之后的参数。直接连接、URL 和连接池各有实际 SQLite 回归。

直接工作线程测试保留内存隔离/共享、绝对/相对文件名、字面 Unicode/百分号/查询字符、URI 选项编码、事务/savepoint/回滚、错误及显式关闭。非 UTF-8 文件名仍在打开原生连接前返回既有 `InvalidData`，不静默扩大文件名契约。

JSON、UUID、regexp、离线元数据、序列化、位向量及 IP 地址类型均显式启用。`sqlite-deserialize`、`sqlite-load-extension`、`sqlite-preupdate-hook`、`sqlite-unlock-notify` 保留能力开关。`sqlite-unbundled` 使用真实系统 SQLite 头与库，不是第二个驱动。整合不恢复 PostgreSQL、MySQL、migration、通用 Any 驱动或运行时/TLS 选择器。

<a id="upstream-provenance-and-validation"></a>

## 上游来源与验证

来源为 SQLx 的 `sqlx`、`sqlx-core`、`sqlx-sqlite` 0.9.0，许可 MIT OR Apache-2.0。原始 [README](../upstream/sqlx-0.9.0/README.md)、[FAQ](../upstream/sqlx-0.9.0/FAQ.md) 和 [crate 文档](../upstream/sqlx-0.9.0/src/lib.md)可用于理解查询与绑定；其中其他数据库、宏、迁移和运行时选择示例不是本仓库用法。原始 manifest、许可证与源码哈希不可变地保留在 `compliance/provenance`；多来源收据记录每份生产文件及可逆外置测试适配。原上游文档不改名。

SQLite 范围门禁构建真实消费者并拒绝删除的 API/特性。保留 regexp 测试使用不依赖运行时的 futures 执行器和隔离内存库，无隐式 SQLx 夹具。第一方测试覆盖超时、回滚、savepoint、Unicode 行诊断、语句缓存容量/驱逐、清空及池关闭。不操作用户数据库。

SQLite ABI 符号、`_sqlx_savepoint_*` SQL 标识及 `file:sqlx-in-memory-*` 连接 URI 有意保留，它们是原生/SQL 标识，不是 Cargo 包名。诊断工作线程使用 `sqlite-worker-*`，无活动 `SQLX_*` 环境选择器。

## 外部字符串边界

准备 SQL 在执行任何前缀语句前拒绝 NUL，避免 SQLite 提前终止解析后留下不消耗输入的尾段。文件名也在 URI 编码之前拒绝 NUL；`immutable` 或 VFS 参数不能让 `%00` 绕过检查并打开截断后的其他路径。显式 `file:` URI 的有效路径/查询部分还拒绝会被 SQLite 解码为 NUL 的 `%00`，VFS 名称拒绝内嵌 NUL。普通字面 `%00` 文件名、URI `%2500`、被忽略的片段及内存库保持原语义。

Collation 两个入口（连接选项与已锁定句柄）均在释放 Rust 自动资源管理之前验证名称的 C 字符串表示；内嵌 NUL 仍返回原有协议错误，不遗留闭包捕获资源。SQLite 注册失败仍由调用方回收尚未交给 SQLite 的闭包，成功注册则在替换或关闭连接时由 SQLite 释放。根 `test/platform/unit/sqlite/driver/collation.rs` 以 Weak 引用覆盖两入口的 NUL 拒绝、合法反向排序及关闭，以及真实活动语句导致的 `SQLITE_BUSY` 注册失败。当前第一方产品未发现自定义 collation 消费者；本修补仅处理公开库 API 的资源所有权，不解决回调 panic 隔离，也不构成整个驱动的安全证明。查询复杂度、同步次数和成功路径的所有权语义不变。
