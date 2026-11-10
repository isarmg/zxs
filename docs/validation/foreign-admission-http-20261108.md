# 外来盘 HTTP 与准确目标写者回归

本切片选择性复用旧 `7564235217b6446ee901b0ec9c8b179339fcff90` 的四项 HTTP 请求准入回归及 fixture；未合并整个旧分支，也不沿用旧测试结果。新增当前执行意图的真实持久化与正常关闭冷重开验证，以及准确目标的运行时 I/O 门禁回归。

## 行为修正

原 `save_foreign_import_intent` 和 `reconcile_foreign_import_intent` 会通过 `retain_writer_locks` 准入全部登记盘的普通 I/O。无关一级盘进入 quiescing 后，即便目标盘仍可用，目标局部意图保存／同步也会错误返回 `DiskUnavailable`。

现在仅这两个控制写路径保留准确目标 writer：核对 writer/root 同一 admission domain、实际 `.writer.lock` FD 身份及空文件约束，保留原 system/selection owner 和目标 writer。调用方的目标活动 lease 覆盖完整读写、同步和错误收尾。普通 publication、quorum、membership 和业务 mutate 的准入规则不变。新增保留为 O(log D) 查找与固定一次目标锁核验，不遍历其余 D 个写者；未声称吞吐或 fsync 优化。

## 精确验证范围

独立 Cargo target：

```text
cargo test --locked --offline -p storage-server --test foreign_admission -- --test-threads=1
cargo clippy --locked --offline -p storage-server --test foreign_admission -- -D warnings
```

运行测试时 `TMPDIR` 必须位于持久文件系统，不能使用 tmpfs；真实管理发现／激活 gate 不因测试而关闭。

- 旧四项：cookie / CSRF / origin、流式请求体后的会话撤销、来源物理选择过期及非法写字段、保存／重验审查后仍拒绝 execute。
- 新四项 HTTP：多来源完整元数据与本机缺正文保持、明确保存确认、完整 review/actor/effect/digest 绑定、原操作错误摘要拒绝、流式保存后撤权、来源移走后的全新服务冷查询与准确原样重试。拒绝及重试均比较真实来源和目标文件字节；冷重试快照从正常重开完成后开始。
- 两项 Engine：通过已有 `fault-injection` 能力对真实磁盘 I/O gate 注入 quiesce。无关盘 quiesce 不阻断目标局部保存和重放；目标本身 quiesce 仍拒绝两者，不新增写入、不越过 ordinary lease。

篡改用例分为不同真实 fixture，保持生产限流，不禁用限流、不通过假身份绕开同一实例限制。门禁注入不是物理拔盘或持久离线流程；正常 drain 后重开不是崩溃或真实断电验证。Router/Backend 请求没有监听 TCP socket，不能称浏览器或网络监听验收。

## 仍未实现

返回仍只有 `IntentSaved` 与 `execution=unavailable`。本切片不启动正文复制，不建立全池 ID 预留，不注册外来盘，不安装多 history 目录，不产生 Native 导入发布回执，也不开放 `plans/execute`。离线 capsule 仍不是活动池接纳。

## 本次实际结果

2026-11-08 在准确主线 `2782b3dba84dda54725cc89bd4a09c2f1fedb18f` 加本切片上重新构建并运行：10/10 通过（8 HTTP/Router、2 Engine gate），该独立 target 的 Clippy `-D warnings`、来源治理 refresh/verify、架构、依赖文档和格式检查通过。包括前置复现、重放到新主线、全部验证及自身产物 SHA 清理的窗口共 839.7 秒，没有运行完整 CI。

修复前在准确 `18fb93c2` 生产代码上，真实“无关盘 quiesce 后原 intent reconcile”按预期复现 `DiskUnavailable`；修复后同一测试通过。最初测试草稿的 `.code`/`.code()` 编译错误不算复现证据；初版篡改用例触发生产限流也不算通过，拆分 fixture 后才计入最终结果。没有运行 TCP 监听、浏览器、真实拔盘或物理掉电验收。
