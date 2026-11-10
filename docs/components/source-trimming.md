<a id="reviewed-production-source-trimming"></a>

# 经审阅的生产源码裁剪

有意移除单个生产 Rust 文件时，使用 `python tools/rust-sources/manage.py refresh --trim-plan PLAN.json`。普通刷新仍拒绝未登记的移除；此机制不是绕过测试迁移或包停用流程的出口。

计划格式为 `{ "schema": 1, "packages": [...] }`。每个包条目仅包含：

- `directory`：活动的独立 `src/foundation/<owner>` 目录。
- `package`：此前 `compliance/rust-sources.json` 目录中的精确、完整归属方快照，包括全部文件哈希。
- `files`：非空对象列表，每项包含 `path`（相对归属方目录）、`sha256_before`（取自该快照）及非空的逐文件 `reason`。

只支持 `build.rs` 和生产 `src/**/*.rs` 文件。测试、基准、示例、清单、合规与许可文件不能通过此方式移除。计划列出的每个文件必须已经不存在。其他被移除的文件仍须按原有显式测试迁移机制登记。计划不授权丢失任何外部测试片段或凭据。

刷新会在 `source_trims` 下追加完整的内容寻址凭据，并在 `source_trim_records_sha256` 下记录哈希。验证会检查凭据完整性、重复路径、归属方身份、当前清单及文件在磁盘上持续不存在。刷新原样保留此前全部凭据。此机制绝不重写原始来源和许可材料。与现有目录历史一样，Git 审阅是外部信任锚；这些哈希是完整性检查，并不是用于抵御有权替换整个目录的一方的签名。

有意不支持裁剪已合入/整合的归属方。对拥有裁剪历史的归属方进行重命名、升级、整合或停用，需要未来专门支持历史链的适配器。另行认证的 Foundation 功能目录布局由其根迁移适配器支持；该适配器保留裁剪凭据，并在当前分组位置检查禁止文件不存在。不要删除凭据或保留无用占位文件来绕过此边界。

治理模块不执行构建，也不写入源码文件。运行 `python -m unittest discover -s test/tools/rust-sources -p test_source_trimming.py` 可执行专项回归套件。
