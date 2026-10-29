# 发布与崩溃恢复有限状态检查器

本工具固定规范基线 `e2f33d9882cf7263412d9370b6a703e0b216357d`，以 Python 标准库实现独立抽象模型。它不导入生产实现、不读真实数据池、不以产品 fixture 或恢复实现作为正确性 oracle。生产源码、格式、Cargo、工具链和现有 CI 均不变。

## 范围与规范映射

具体状态、转换及抽象取舍见 [SPEC.md](SPEC.md)，独立审阅见 [REVIEW.md](REVIEW.md)。

| 契约 | 基线规范 | 检查位置 |
| --- | --- | --- |
| 固定 N=1/2/3/4、Q=floor(N/2)+1，唯一写者与候选、顺序槽位 | docs/10 §10.2、§10.5；ADR 0002 | model.py 的配置/转换；properties.py 的独立性质 |
| ACCEPT 决定不同于全成员保护、PUBLICATION、公开与确认 | docs/10 §10.1–10.6 | 分阶段屏障、独立发布前提及响应性质 |
| 文件 bytes、完整性、文件同步、目录项与目录同步分别建模 | 根规范 §7；docs/10 §10.3 | Artifact 与进程/断电转换 |
| 不可读/损坏不代表完整日志无记录，恢复至少 Q 个可信旧成员 | docs/10 §10.6–10.8；docs/18 | 恢复观察与恢复中止性质 |
| 原 ID 重试、摘要绑定、公开 State/永久结果/事件一致 | docs/10 §10.5、§10.10；docs/18 | 事务历史与重试性质 |
| 历史保护事实与当前健康分离；派生索引失败不撤销发布 | docs/10 §10.10；ADR 0005；docs/18 | 历史事实、健康、索引状态与边性质 |
| 掉盘后不能缩小成员集正常写入 | docs/10 §10.2、§10.11；ADR 0002 | 固定配置及后续槽位准入 |

规范链接均指基线仓库中的对应文件；本目录不复制另一套产品规范。

## 分工与执行位置

实际启动了三个独立代理：protocol 编写转换和 SPEC；search 编写独立性质、BFS、重放及搜索自测；review 核对规范、审阅两者并编写破坏规则验收。集成协调者负责运行矩阵、隔离 GitHub 验证载体、证据与交付。

云工作区只读/编辑源码、使用 Git 和静态语法检查。正式自测、搜索及反例重放只在 GitHub Actions 运行。专用 workflow 的来源模板放在 `test/publication-model/validation.yml.template`；实际 workflow 在无产品祖先的独立 orphan 验证分支中，checkout 精确工具提交。不要把验证载体合并回产品。

## 可重跑命令

要求 Python 3.11+，无额外依赖。从仓库根运行：

```sh
python3 -m unittest discover -s test/publication-model -p 'test_*.py' -v
python3 test/publication-model/run_validation.py --output /tmp/publication-model-results
python3 tools/publication-model/cli.py search --members 1 --operations 1 --replicas 1 --crashes 0 --max-states 50000 --max-transitions 500000 --max-depth 120 --max-seconds 30 --output /tmp/search.json
python3 tools/publication-model/cli.py replay /tmp/publication-model-results/counterexample-shrunken_quorum.json
```

运行矩阵先枚举小规模，再扩大到 N=3/4；每档记录操作数、崩溃、故障预算、深度、状态与转移数及所有资源截断。`BoundedExhaustive` 仅表示该明确有限抽象的队列耗尽；`Inconclusive` 不算通过。破坏规则必须搜索得到 `Counterexample` 并成功重放。原始 stdout、stderr、环境信息、精确工具/验证提交、各档 JSON 和反例均作为 Actions artifact 上传。

## 规范歧义与证明边界

基线 `docs/adr/README.md` 对 ADR 0005/0006 的状态描述与 ADR 0005 正文及 docs/10 §10.3 的“尚未启用”不同；`docs/architecture/write-flow.md` 使用“三成员”，而编号规范 docs/10 明确 N≥1。此模型采用任务明确指定的 N=1..4 与编号规范的固定 N/Q、完整新写入保护屏障；不推断免旧依赖读取已经启用。

模型不涵盖成员替换、选主、恶意写者、GC、checkpoint 裁剪、真实字节编码/哈希碰撞、实际文件系统/设备缓存行为或性能。内容仅有每操作一个版本、1 或 2 个独立完整副本。最多两槽位及两次崩溃。故障预算、操作深度和搜索资源是独立边界，结果必须同时披露。不能据此声称无限规模、真实掉电或整个产品正确性已获证明。
