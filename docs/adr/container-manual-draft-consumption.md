# 容器共享手动稿与显式消费（WIP）

状态：隔离分支实现中，未经过定向运行验证，不是 main 验收。

## 权威与边界

- 必填 SYSTEM `container_edits` 是模式与当前稿的唯一决定权威。DISK 的
  immutable draft body/intent 只证明材料，不能自行成为当前稿。
- 缺少整张必填表不是自动模式兼容入口。表内尚无记录的 FileId 才是初始
  Automatic/0。最初检查点仅快照 schema 拒旧；后继明确 fresh 入口加入首写 selector/
  DISK discriminator，细节见文末。普通旧入口退休仍未完成，不能声称旧 writer 全部退出。
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

## 独立 fresh 格式后继（未验证）

`initialize_manual_container_storage` 仅接受调用者已经创建的私有空 SYSTEM 和独立
level-1/无 upstream 的根。SYSTEM 与所有 DISK 原目录锁、FD 在首写期间持有；
`initialized.json` 使用原旧 writer 已拒绝的槽名直接 CreateNew，再 sync 目录和正文，
不是旧 writer 不认识的临时文件 rename。selector 固定原 operation、config commitment
与全部物理根身份；新 DISK seal 使用独立未知判别并绑定同一 commitment。

selector 是格式选择证据，DISK 副本不能自授当前稿，SYSTEM 永久 Candidate 仍是稿决定。
完整初始化后 Complete 记录绑定同 selector、原 SYSTEM/DISK writer inode 及 bootstrap-discovery 目录/writer 身份。
manual 的 bootstrap cold hold 只打开既有 lock，不创建或同步替代 sidecar。cold open
先严格验 Complete、根实体、selector 副本及所有既有 writer，之后才进入原恢复路径。
缺 Complete、丢失/替换 writer、未知/旧/A selector 原字节保留并拒绝，不重建 sidecar。
marker 后任一未知 partial 当前只保留拒绝，不宣称自动完成每一个初始化崩溃窗口。

普通旧初始化入口与旧材料 reader 的全部退休仍待单独收口。本阶段提供明确新入口，
绝不向已存在的池加 marker；不是并行旧格式作为最终兼容交付。新模式拒绝 legacy
membership generation 变换，不扩展多级备份。继承的 SYSTEM SQLite/locator 操作尚有
绝对路径，首写/准入的持有证明不等于全链 held API；未完成该边界与 GitHub 验证前不进 main。

### bootstrap 预准入到实际锁的证明传递（未运行）

冷开配置保留原 Complete 的 typed commitment，不以稍后重读的本地记录自证身份。
实际 bootstrap 目录 FD 必须匹配该原记录；随后 Existing 锁 FD 必须匹配原 writer
inode、空正文及锁后路径身份。没有该运行时证明的旧管理入口拒绝新格式。
新增接缝回归分别在预准入后替换 bootstrap 根，以及取得根准入后替换 writer，
要求失败且完整磁盘 bytes/inode/mode 不变。仅静态审查，未执行测试；这不代表
SYSTEM SQLite/WAL 或全部恢复路径已经改为持有式访问。

### 冷开 saved Config 的格式证明传递（未运行）

固定 locator probe 仍先验原盘实体、原登记和固定 quorum。随后从 DISK 解码的 Config
本身不含 manual 权威；仅原已完成 SYSTEM 准入可以将其与相同 canonical selector
及新 seal 绑定，派生用于该次 held seal 比较的配置。原 seal 全字段相等检查、voter
集合、locator restriction 及后续 publication/current 验收不变。无完成证明或旧 seal
拒绝，普通旧 Genesis 入口不获得升级权限。已有实际 FILE cold/read 与 draft 冷开回归
保持；初始化回归扩原证明缺失、旧 seal、改坏 selector 全盘不写证据，尚未运行。
