# ZXS 内部管理 HTTP 客户端

此包只处理远程业务 API、会话和文件传输，不打开存储池或调用 Linux 平台层；当前保留为独立库及回归消费者，产品守护进程和离线救援入口均不依赖它；Web 直接使用浏览器 HTTP API。

| 模块 | 职责 |
| --- | --- |
| `lib.rs` | 服务器 origin、会话、通用请求、封存/摘要/提交、冻结目录分支查询和显式批量分支映射入口 |
| `response.rs` | 有界 JSON 响应、空成功响应、保留 HTTP 状态的错误映射 |
| `transfer.rs` | 流式上传及完整性验证后的下载协调 |
| `download_target.rs` | Linux 描述符绑定的暂存、无覆盖发布与清理 |
| `transfer_tests.rs` | 临时本机 HTTP 下的下载、并发目标及错误回归 |

服务器可使用 HTTP 或 HTTPS origin，不限制 HTTP 主机为回环。HTTP 不保护传输中的凭据与数据，网络隔离及 TLS 部署由用户管理；HTTPS 仍验证服务器证书。禁止凭据 URL、重定向和跨 origin 请求。此客户端库与 Web 共用 `storage_contract::validate_api_path`，在 URL 解析前拒绝原始/编码点段、控制字符与分隔符混淆，解析后再检查 origin 与 `/api/` 前缀。查询参数单独处理，不作为路径段校验。

Linux 下载在联网前打开并保留目标父目录，在其中创建私有暂存目录，流式验证长度和 BLAKE3 并同步文件，再通过持有的文件描述符一次性无覆盖发布。父目录或临时路径被替换不能改变发布的内容；目标已有普通文件、目录或悬空链接均不得覆盖。发布任务派发前，取消清理绑定原有暂存目录和文件身份，不跟随替换路径。创建暂存目录后若尚未取得目录句柄就失败，宁可保留空目录，也不按未经验证的路径清理。

下载的强 ETag 在响应边界解析为共享 `ContentHash`，只接受双引号包围的 64 位小写十六进制正文摘要；非文本头值与无效摘要分别保留具体 `source`。正文完成后直接比较定长摘要，不保留摘要文本或再次编码十六进制；弱 ETag、缺失头及非法格式均在创建暂存文件前拒绝。

文件校验及同步完成后，客户端将无覆盖发布交给 `spawn_blocking`。任务一经派发，销毁或取消下载 future 不会中止该任务；目标可能已经或随后发布，但调用方未收到完成结果。目录同步失败也可能发生在目标已建立之后。因此此阶段的取消或错误不证明“目标不存在”或“未发布”，不得据此删除已有目标或盲目重试；应先核对原目标及其完整内容。这里说明现有结果不确定性，并未提供新的取消、后台任务 drain 或持久化完成保证。

该发布路径需要 Linux 的 `/proc/self/fd` 可用；不可用时失败关闭，不回退到按临时路径发布。其他平台不提供此下载保证。内部随机名不拼接目标名，合法长文件名不会被内部前后缀挤出文件系统限制。下载内容是从远端独立复制的完整字节，不与服务端版本或其他下载去重。

上传只接受已打开句柄指向的普通文件。Unix 打开源文件时使用非阻塞标志，避免无写入端的 FIFO 在类型检查前挂住；指向普通文件的本地符号链接仍可上传。

完整上传先使用 256 KiB 缓冲计算摘要并回退文件位置，释放摘要缓冲后才构造按需读取的请求体。请求体容量按文件长度限制在 4–256 KiB，不创建后台预读队列，也不缓存整文件；大文件相比默认 4 KiB 容量减少数据分块数量，但不代表已测得网络吞吐提升。此路径属于保留的客户端库，不再由产品 CLI 暴露，也不是桌面或移动 App，也不改变服务器缓存。回归在 `test/client/unit/upload_stream_tests.rs`。

会话序列化用于登录业务，`Debug` 只显示主体和期限，不输出 bearer/CSRF。空成功响应包括 HEAD/204 返回 JSON null；非 JSON 的 HTTP 错误保留其状态。`Error::Api` 将 `request_id` 独立保留为 `Option<String>`，`message` 不混入请求号；缺失请求号保持 `None`，不生成替代标识。Display 仍在存在请求号时附加诊断后缀，调用方应直接读取结构字段而不是解析显示文本；响应解码的具体 `source` 链不变。提交超时仍需查询原操作 ID，不能把传输错误解释为未提交。

```sh
cargo test --locked --offline -p storage-client
```

HTTP 回归使用临时目录和本机随机端口，不访问已运行用户池，不代替 Linux 文件系统故障、跨浏览器或 SMB 客户端设备验收。

## 业务便利入口

这些方法只构造并发送当前协议请求，不自动重新读取基线、生成 operation_id 或重试；当前身份、对象归属、名称／路径与动态冲突仍由服务端准入。写入响应不确定时，先按原 operation_id 查询；如重试，保持原身份、operation_id 和全部输入，不能用新读到的 head、scope 或代数替换原请求。通用 `request` 可用于便利方法未表达的当前协议字段，不提供旧格式兼容。

| 方法 | 调用合同 |
| --- | --- |
| `directory_branches` | 只读冻结目录分支，不切换默认分支；`path: None` 查询所有 scope，`Some("")` 只查询根，非空 path 查询指定目录 |
| `directory_namespace_batch` | 原样发送显式源／目标目录分支、冻结 tree 与条目身份，并绑定调用方 operation_id；不自动解析或补齐映射 |
| `rename_file_branch` | 修改指定公共文件分支标签，不修改内容或当前选择 |
| `create_file_branch_from_node` | 同时固定节点与正文 version；服务端检查二者属于同一文件且正文对应，不自动选择新分支 |
| `set_file_path_note` | 名称保留，但也接受标签／合并元数据节点；空字符串显式清空该节点备注，不修改正文版本备注或新增节点 |
| `merge_file_branches` | 固定每个输入分支 head、所选结果与归属分支；调用即发送 `confirmed:true`，调用方须先取得用户明确确认 |
| `update_file_node_tags` | 固定每个源节点、关联分支与预期 head，发送 add/remove 增量，不改写调用方输入 |
| `backup_configuration` | 读取指定路径的自身／有效策略与 scope；空路径表示仓库根 |
| `configure_backups` | 固定原 scope 与**自身 `generation`**（不是 `effective_generation`）；总份数包含主盘，备份前缀原样发送，代数编码为规范十进制字符串 |

`merge_file_branches` 与 `update_file_node_tags` 不发送 `repository_id/path` 条件；核心将这一缺省位置明确绑定为 `location:null` 的摘要，不能将其理解成对某个逻辑路径的条件写入。需要位置约束时，通过 `request` 同时发送这两个字段，并在重试时保持它们不变。详见[请求摘要与幂等](../15-api-and-errors.md#s15-07)。目录分支合并待用户确认，暂不实现且无业务入口。

历史节点合并使用 `POST /api/file-manager/files/{file}/merge-history`（客户端 `merge_file_history`）。输入为 `{branch_id,source_node_id,expected_node_id}`，可选择同一分支的不同历史节点，包括只读合并别名；`chosen_node_id` 选择同文件结果，`name` 明确新结果分支名，`confirmed:true` 明确提交。一次 mutation 派生并合并新分支，全部原分支、较新 head 和当前路径引用保持不变；不自动切换默认分支、不复制正文。辅助分支以 operation ID 和输入序号稳定命名，冲突原子拒绝。客户端便捷方法不发送位置条件；需要路径约束时通过 `request` 成对发送 repository_id/path。重试保持全部输入与 operation_id，详见[历史节点合并](../15-api-and-errors.md#历史节点合并)。

`Client::directory_history(&DirectoryHistoryRequest)` 读取目录节点或分支页，使用共享 `route/query_fields` 形成请求，并以 `DirectoryHistoryPage::validate` 校验响应。调用方在节点/分支页之间保持同一精确 root，root 改变后重取而不拼接。目录树不提供修改授权或目录合并。详见[目录只读历史窗口](../15-api-and-errors.md#目录只读历史窗口)。

`Client::file_history_source(&FileHistorySourceRequest)` 按需解析旧节点的真实来源分支，返回已有 `HistoryBranchSummary`，node_id 是提交时的 reviewed head。它只服务用户点选，普通历史页面无需调用；字段由 `storage_contract::file_history_merge` 统一解析与验收。
