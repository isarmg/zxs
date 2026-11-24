# Bearer 元数据只读

状态：[仅静态]。此页只增加显式 Browse 客户端可用的权限，不改默认客户端、存储、旧 Basic、原操作回执或正文协议。登录使用[相同账户会话](sessions.md)，通过已验证的 HTTPS/HTTP/2 门面。Bearer 不授权旧 `/data`、任何旧 R3 写操作或尚未开通的上传。

## 精确请求

下列路径统一以 `/zxs/v1` 开头，保留原大小写和拼写；仅 GET，不支持查询或别名。C、N 和非 `main` 的 B 是正则形式为 32 位小写十六进制的非零标识。C 是容器，N 是不可变节点，B 是分支。

| 路径 | 必须和可选的业务头 |
| --- | --- |
| `/containers` | `Zxs-View: compact`；可选 `Zxs-After: C`。 |
| `/containers/{C}` | `Zxs-Detail: paged`；可选正十进制 `Zxs-Detail-Sequence`；`Zxs-Detail-After: B` 仅可随确切序号一同提供。 |
| `/containers/{C}/branches/{B}/nodes` | `Zxs-History-Head: N`；可选 `Zxs-History-Next: N`，不可等于声称的头。 |
| `/containers/{C}/tags` | 不接受任何 `Zxs-*` 头。 |

对这些新 Bearer 读，授权头恰好一个；正文无内容，`Content-Length` 可省略或恰好为单个规范的 `0`。不接受 Content-Type、内容编码、Range、Content-Range、传输编码、trailer、重复控制头或其它 `Zxs-*`。无效帧不会进入 Store。已认证但当前仅 Basic 才允许的合法旧业务请求返回空 403，不读取或排空其声明的正文。两种授权共用旧 URL；无授权的旧请求继续收到原 Basic 质询，不构成任一客户端的降级许可。

## 原样返回的值

成功是原 R3 200 JSON，`Cache-Control: no-store`，最多 1 MiB、超限报错而非截断。以下键不增加别名或新包装：

- 紧凑列表是 `items,next_after`；项目由 `container_id,sequence,name` 组成。
- 详情是 `container_id,sequence,name,tags,heads,next_after`；每个头是 `branch,head`。后续页严格核对原序号。
- 历史是 `head,nodes,next`；节点仍包含 `node_id,operation_id,parent_node,original_branch,sequence,length,sha256`，不含正文；后续页核对指定的当前头及祖先。
- 标签是裸字符串数组，包括空数组，无额外键/分页。

列表、详情头和历史各自每页最多 256 行；详情的标签不是独立分页。冲突沿用原头或原序号的 409，不自行改读新头。其它错误沿用旧语义：缺失为空 404、无法核定为 503，错误不是部分成功。显式 Bearer 无效或到期是空 401、仅 Bearer 质询；服务停接纳不把它误报为另一种登录。原回执与区间读取的既有协议保持不变。

每一项从同一账户与服务纪元重新证明有效授权，读 Store 前、完整投影后和首次响应写出前再次复核。只有完整成功交换才按现有后端有界顺序规则复用；拒绝或失败后关闭，不扫描后续请求。客户端需显式选择 Browse，只发上述只读请求；不自动改认证、重新登录、重发或换一条线路。
