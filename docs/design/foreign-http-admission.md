# 外来盘 HTTP 准入与审阅权限边界

## 本轮范围

本轮基于主线 `f6761067a07a71065540c5ca006eca32ade40aa8` 的同树本地提交
`b0eb510ed66cd652b2722ef9806f874ea803f4a5`，只增加独立
`foreign_admission` HTTP 集成测试目标，不改生产权限或导入实现，不改变
`phase1_save_tags` 的既有 12 项测试清单。

外来 SQLite 严格 schema 检查的独立原型 `026626dd7a2e9050a6bc1c8d6252ef79bada0c0a`
及其捕获/归一化父链不在本次生产图内。原型检查通过不能签发当前 HTTP 接口的
发布证明、成员资格或写权限，也不能当作外来盘端到端导入完成。

## 真实路径及保证

1. `setup_recovery/http.rs` 把管理入口置于真实 HTTP 会话、CSRF 与同源准入后；
   严格 JSON 解码、请求体上限、超时和管理槽仍生效。
2. `ManagementBackend::setup_operation` 在访问数据路径前重新核对当前会话。
   外来预览、计划预览/保存/再核对共用存储生命周期互斥；受保护恢复状态不会
   退回普通新盘探测。
3. `storage_directories::reopen_source` 通过固定发现根重新打开持有目录，核对
   本机来源与发现根身份、挂载和已登记/停止目录边界。该本机观察不是便携身份
   或后续写入授权。
4. 外来预览调用真实历史检查，返回 `execution = unavailable`。未能读到的正文
   不因此变成应剪除的历史；本轮 HTTP 夹具只选择完整的一级来源，未新增多级
   缺正文覆盖。
5. 保存计划只把明确确认的审阅写入系统盘不可变审阅记录。再次核对的
   `matched_at_observation` 不是预留、权威发布或执行资格。
6. `/api/admin/storage/foreign/plans/execute` 即使拿到正确审阅选择也拒绝执行。
   客户端额外的 `execute`、`publication_proof` 或伪造 `ready` 不能升级权限。

## 新增回归

- 无会话、缺 CSRF、跨源请求均拒绝；正确 cookie 请求仍可进行只读预览。
- 用流式请求体和两个一次性通道固定交错：入口完成首次认证、等待 JSON 后半段时
  通过真实 logout 撤销会话，再恢复请求体；后台必须重新拒绝。没有用 sleep
  推测竞态发生时机。
- 旧 inode 选择、发现根外路径、额外写字段和查询参数拒绝，之后原有效选择可重试。
- 激活本地目标后，真实预览、保存审阅、再次核对全部完成，执行仍拒绝；伪造
  `ready` 不能替换已有审阅。源盘、目标盘和已保存审阅的文件字节保持不变。

所有数据均由实际 Engine 和 ManagementBackend 在临时持久文件系统中创建。
测试使用真实 AuthStore、路由和业务调用，没有替代 backend、伪造发布签发器或
私有测试 hook。文件字节前后对照不等于未发生读取系统调用的证明。

## 验证入口与限制

正式目标可用根工作区及锁运行：

```sh
TMPDIR=/持久文件系统上的私有临时目录 \
  cargo test --locked -p storage-server --test foreign_admission -- --test-threads=1
```

夹具保留真实持久文件系统准入，不能用 tmpfs 绕过。本地受控验证复用准确
b0eb 默认 Engine 和 default/core Server，只链接本次测试源码；它不等于正式
Cargo dev-feature 合并后的 fault-injection 图，也不代替浏览器、网络客户端、
物理盘掉电或实际导入验收。准确命令、源摘要、非零枚举和运行结果单独封存。

本次受控本地验证已枚举并串行运行全部 4 项，4 通过、0 失败、0 忽略，严格
Clippy 通过。7 个复用库重新核对字节摘要，515 项已登记构建输入与准确 b0eb
Git 对象一致。首轮保留 3 通过、1 夹具失败证据：夹具错误猜测目标槽为 P01；
修正为激活后读取真实磁盘列表中的唯一 primary 槽，不改变生产实现。
正式 Cargo 目标、完整 dev-feature 图与 CI 本轮尚未运行，来源 manifest 的
官方 refresh 由完整源整合视图完成后方可声称通过来源治理。

待完成的产品接线仍包括完整来源/层级证据与作用域投影、真实 publication 对
元数据根和安装依据的承诺、可恢复导入决定及持久安装。SQLite checked 类型、
审阅摘要、当前主机 inode 和保存计划均不能补足这些缺口。
