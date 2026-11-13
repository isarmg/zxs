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

当前小片接 native 及 PublicCommit/wire 的普通主目录 workspace。
显式 `seal_container_draft_workspace` 使用调用者提供的原 operation，建立持久绑定并在
SealedWorkspace 返回 typed proof；普通 `seal_workspace` 绝不自动建立绑定。公开请求的
proof 必须精确等于 workspace；manifest 与 request digest 都绑定它。修改 proof/op 即使
重算请求 hash 也拒绝。冷重试发送原冻结 envelope，不重新 seal 或重写 operation。
历史分支的专门 historical-save 流程仍明确拒绝并保留稿，不能换入口自动建分支。
server 普通 SMB flush 不调用此准备方法；HTTP 显式准备路由尚须独立接线和验证。

## 验证计划与未完成项

现有真实 engine 回归扩展：两次保存/冷读不改历史；普通提交无绑定拒绝；显式绑定后
冷开提交、清稿与新版本同决定；再冷开 exact operation 重试无新发布；旧历史正文不变；
显式丢弃后冷开无稿；直接读安装 FILE SQLite 验证单字段 edit 更新。

只执行静态阅读及 `git diff --check`，没有本机编译、测试或 formatter。
后续 GitHub 定向组 `container-manual-drafts` 才提供运行证据。仍需完善故障边界、
历史提交、HTTP 显式准备路由、全链权限/格式早拒、clean package 导入边界与稿回收。

最终格式必须与独立目录根/目录 worktree 格式统一审阅。本分支不把 A 分支的 selector
当成手动稿授权，也不向已有 A/legacy 根补字段迁移；未证明格式及消费者整合前不进 main。

## 从共享稿直接准备（后继 WIP）

`prepare_shared_container_draft_commit` 以真实当前稿和原 EntryId 开启普通单文件 workspace，
先持久同 operation 的准备绑定，再通过 `UploadSource::Managed(DiskFile)` 复制当前正文，
最终显式 seal。传输保留原源盘责任，不克隆逃离 gate 的裸 FD；fingerprint/copy 每个
checkpoint 都检查源盘与目标盘撤销。正文使用已有 workspace 总量/单稿预算。
同 operation 已绑定 workspace 可继续准备；若崩溃发生在 workspace 建立但绑定未落盘的
窗口，该未知 workspace 保留、不自动收养/删除，不声称所有中断均自动恢复。

新增原源盘与目标盘不同的实际 FD 回归覆盖复制前/复制中撤销，要求原正文不变、源描述符
关闭而目标责任仍受其自身 gate 管理。公共显式提交回归直接消费这个 producer，不经浏览器
下载再上传。完整主动撤销/断电矩阵仍须 GitHub 验证与后继补充。
