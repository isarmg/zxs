# 重复纯添加：后继代际实验批次

本批源码在首次激活基础上实现后继权威链。首次批次的[GitHub 成功证据](first-membership-activation.md)只覆盖首次添加，不覆盖本批。[重复批次首次功能运行](https://github.com/isarmg/zxs/actions/runs/41812908199)中 22 个激活测试通过 18 个，4 个多代测试因恢复入口把已归档前代封印误判为当前日志缺失而失败；生产、Clippy、静态、契约和其余受影响过滤通过。现已将豁免条件收窄为认证历史索引中的精确 sequence/hash 匹配，并增加当前日志缺失的反例，修复尚待 GitHub 验证。未执行本地编译或测试，不能据此合入 main 或开放在线 API/Web。

## 已编写、待 GitHub 执行

- 连续元数据成员 2→3→4：每次各自固定旧/新 N/Q，后继显式引用前驱，epoch 1→2→3 与拓扑独立，永久结果逐条不变。
- 第二次 marker、seal、Prepare、Accept、Finalize、Publication、系统配置投影前七处中断；显式恢复后只能完成同一后继，不能退回冻结 State。
- 第二次 fence 仅落一名旧成员后冷重启：保留只读状态与同操作恢复，尚未登记的新增空根不写入。
- 原正文、旧本地 proof、冻结旧日志不变；只保留 data-only 根时分别导出 Genesis、第一代、第二代正文，旧和最新历史包独立检查。删除便携前驱节点明确失败，不回退 Genesis。
- 丢失系统目录、移动目标成员目录并只提供 3/4 目标多数：恢复最新已发布结果，缺员保持禁止新写。
- 一个目标成员的祖先 archive root 损坏：保持 N=4/Q=3，隔离该成员，保留其所有文件字节并给出恢复警告，其余多数仍读取最新结果。
- 连续两次添加后，清空当前代日志或截断到激活已发布前缀，即使后续完整 publication seal 仍在，也必须拒绝恢复且不凭封印重造决策历史；原日志截断字节和封印均保持不变。
- 既有首次激活 17 组及受影响成员、恢复、降级、proof、scanner、workspace、history package、scrub、identity、provenance 过滤继续执行。

复用现有 [membership-validation workflow](../../.github/workflows/membership-validation.yml)，不新增完整 CI 或修改触发规则。新增测试仍使用 `membership_activation_` 前缀；具体测试计数、编译、生产、Clippy、格式、架构和契约结果必须绑定本批提交与 Actions 运行后填写。

## 边界与成本

实现设计见 [ADR 0013](../adr/0013-generation-bound-membership.md)。每份完整控制归档使用一个平铺内容哈希对象树，祖先日志段独立命名；各保留过渡归档之间仍可能复制历史控制内容。便携权威仍受现有单对象 64 MiB 上限预检，没有任意成员规模、任意代际深度或吞吐承诺。

本批只允许新增的每块盘均为 metadata voter，至少新增一名 voter；不支持仅新增 data-only，也不支持混合新增 metadata 与 data-only。保留既有 data-only 根及其证明不等于支持新增 data-only。现存路由、角色、绑定和设置保持不变。这是本批未完成边界，不是最终产品限制；下一独立批次将处理 voter 集合不变的纯 data-only 及混合新增，必须补齐非投票新盘身份、seal、恢复与独立证明，不只放宽布尔校验。

不实现计划下线、删除、正文排空、设备退休返回或 HTTP/Web。临时目录故障注入不等于物理掉电、跨主机 fencing 或恶意克隆/回滚验收。禁止用新格式或新代际重解释旧 Candidate、重置运行池或删除现存用户数据。
