# 容器共享手动稿与显式消费（WIP）

状态：隔离分支实现中，未经过定向运行验证，不是 main 验收。

## 权威与边界

- 必填 SYSTEM `container_edits` 是模式与当前稿的唯一决定权威。DISK 的
  immutable draft body/intent 只证明材料，不能自行成为当前稿。
- 缺少整张必填表不是自动模式兼容入口。表内尚无记录的 FileId 才是初始
  Automatic/0。当前仅快照 schema 拒旧；启动首写 selector/DISK discriminator
  的零副作用早拒尚未完成，不能声称旧 writer 全部安全退出。
- 稿不是 VersionId、历史节点或分支；FileId 共享，原 EntryId/branch/node/version/
  selection generation 绑定来源。容器改名不得通过猜 path 重新关联身份。
- 普通 flush/close 不获得显式消费许可。目录提交、未提交 worktree 软删与手动稿的
  交界仍待统一决定，本片不重定义这两项既有语义。

## 当前显式提交小片

`prepare_container_draft_commit` 只绑定已经 sealed 的单文件普通 workspace。
它要求真实当前稿、精确 edit generation/draft operation、原来源 EntryId、相同正文
hash/size、原 workspace owner/fence/base head，且没有 namespace operations。
读回当前稿必须验证正文与原来源，失败不以历史正文代替。

绑定 `DraftConsumption` 随 workspace 持久，进入 native request digest 与永久 receipt。
既有内容提交计划器复验绑定；成功的新内容节点与 ManualDirty→ManualClean 在同一
Candidate/SYSTEM 决定发布。冷恢复依同一 receipt 证明，不从本地 workspace/body 猜测。
普通提交即使正文相同，也不能自动清稿；修改已绑定请求不能复用原 operation。
同正文显式提交允许沿既有 no-change 规则清稿，不制造无意义历史节点。

仅有 typed receipt 且前后精确稿代数/完整 revision/结果内容相符的 FileId 才可绕过
脏稿来源不变围栏；不是任意 `workspace.commit` kind 字符串豁免。

当前小片只接 native 普通主目录 workspace。历史分支的专门 historical-save 流程与
PublicCommit/wire sealed envelope 尚未闭合，明确拒绝并保留稿，不能换入口自动建分支。
server 普通 SMB flush 不调用此准备方法；HTTP 显式提交尚不能因此宣称已接通。

## 验证计划与未完成项

现有真实 engine 回归扩展：两次保存/冷读不改历史；普通提交无绑定拒绝；显式绑定后
冷开提交、清稿与新版本同决定；再冷开 exact operation 重试无新发布；旧历史正文不变；
显式丢弃后冷开无稿；直接读安装 FILE SQLite 验证单字段 edit 更新。

只执行静态阅读及 `git diff --check`，没有本机编译、测试或 formatter。
后续 GitHub 定向组 `container-manual-drafts` 才提供运行证据。仍需完善故障边界、
PublicCommit/历史提交、全链权限/格式早拒、clean package 导入边界与稿回收。
