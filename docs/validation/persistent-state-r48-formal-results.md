# R48 persistent State：真实 Engine 配对结果与不采用决定

日期：2026-10-27。**决定：保留 baseline 的容器实现，不回退后续已合入的项目改进；当前 AVL 候选不合入 main。** 冻结比较仍是下述精确 R47/R48 pair，不表示将 main 回退到 R47。正确性/证据准入通过不等于性能采用。部分 held resident 和 snapshot clone 确有下降，但本轮未建立稳定端到端写收益，并出现多条两种执行顺序均超过预设 10% 读回退线的路径。没有新增测量、重试、剔除异常或自动扩到 1k/10k/100k。此结论不表示 AVL 永远更差，也不否定尚未测量的大规模收益。

## 1. 精确边界与进程数

- baseline：`bcf03ce6cd25e96e3bcac6600348efdda07e13f6`，tree `016f45b0b4afcbb9514be0fc9bb6a0fd6e278f18`；[normal CI 40509717658](https://github.com/isarmg/zxs/actions/runs/40509717658) 17/17 成功。
- candidate：`185fc97ee483eb15732c36721d9ba7600d727fd1`，tree `3869faa688995ddacd383af5236fd550f6f54cf1`；[normal CI 40513845544](https://github.com/isarmg/zxs/actions/runs/40513845544) 17/17 成功。
- 正式执行源：`14354fbc2822a986415c4473010773a6e39e1d87`，tree `ba1746eb576f7463a6f78ebfc6f0130831a60480`。实验分支 workflow 永不合入 main；后续 descriptor 优化不属于该 pair，未移动 baseline。
- 同 C-format/typed keys/general owner delta/name/index/provenance/server pipeline，仅 State maps/operations/events/auth 容器及等价适配不同。baseline events 为 binary partition + borrowed slice iterator，不是 linear skip。
- 合法 **full revalidation / omission-disabled**：没有伪造周期 scrub owner 或省略授权。warm rename 的精确 owner-delta 源条件及真实 API oracle 已审，但没有逐样本动态分支计数；不宣称最终端到端 O(delta)。
- **100 formal processes**＝5 suites×2 shapes×5 paired repeats×2 arms；另 **20 同 binary admission processes**＝每 formal job 新构建后 4 cases；再另 **20 先验 pilot processes**＝五个较早独立 pilot jobs。合计 140 个 R48 完整进程，只有 100 个 formal 进入主配对统计。旧 R37 三次 attempt 不混入。
- 每 suite 在一个 standard Ubuntu 24.04 hosted runner 上顺序执行，双 clean worktrees/独占不同 targets，工具链/profile/lock 一致；每 shape 顺序 AB/BA/AB/BA/AB（3/2，非完全平衡）。32 个进程内 query 或8次 capture 不算独立重复。

|suite|先验 pilot|formal（含新4-case admission）|formal cases|
|---|---|---|---:|
|save_fixed|[40520958337](https://github.com/isarmg/zxs/actions/runs/40520958337)|[40526872615](https://github.com/isarmg/zxs/actions/runs/40526872615)|20|
|save_growing|[40521874910](https://github.com/isarmg/zxs/actions/runs/40521874910)|[40527132872](https://github.com/isarmg/zxs/actions/runs/40527132872)|20|
|rename_fixed|[40522175300](https://github.com/isarmg/zxs/actions/runs/40522175300)|[40527348756](https://github.com/isarmg/zxs/actions/runs/40527348756)|20|
|rename_growing|[40522336457](https://github.com/isarmg/zxs/actions/runs/40522336457)|[40527607227](https://github.com/isarmg/zxs/actions/runs/40527607227)|20|
|auth|[40522482612](https://github.com/isarmg/zxs/actions/runs/40522482612)|[40527917679](https://github.com/isarmg/zxs/actions/runs/40527917679)|20|

## 2. 实际规模，不以64这个名义值替代

每进程1 repository/1 history。save 做64次真实 create_workspace/stage/commit；rename 先64次真实save、真实file-branch创建，再64次真实 rename（新OperationId、每次新合法label）。

|suite / auth|branches|commits|Σcommit entries|versions|endpoint ops/events|live files|State canonical B|auth canonical B|
|---|---:|---:|---:|---:|---:|---:|---:|---:|
|save_fixed|1|65|64|64|65/65|1|210330|0|
|save_growing|1|65|2080|64|65/65|64|748758|0|
|rename_fixed|3|67|66|64|130/130|1|277379|0|
|rename_growing|3|67|2082|64|130/130|64|815837|0|
|auth / 0|1|65|64|64|66/66|1|211257|185|
|auth / 64|1|65|64|64|66/66|1|218424|7352|

非auth suites 的0表示未配置 auth，不能冒充测得0-session AuthStore。auth suite 实际 initialize/login×N/authenticate/snapshot，容量64、TTL12h、max4MiB不变；0/64 session分别185/7352 B，无人工filler、伪session或静默调额。密码hash/login属于setup，不算write；本文和索引不复制生成的password/token。rename恢复后首个新写另计，最终ops/events变为131，commits/versions不变。
每池三个真实member目录，不是三块物理盘。root路径/inode/seal及每member readback保留；没有移植seed、reflink/hardlink伪副本。随机ID/layout使metadata objects/member有实际差异：save-fixed 942–944、save-growing 1060–1083、rename-fixed 1693–1695、rename-growing 1814–1832、auth 948–951；不声称两臂文件bytes完全相同。

## 3. 预定主指标：十个shape的全部五对

每格百分比为五个配对百分比的中位数，不是合并进程后的比值。写为最后8次真实 Engine 调用的进程内中位数，再配对；正数更慢。held8在最后8次操作前捕获1→8个旧State。全对原始值、18项指标、AB/BA和离散见artifact `formal/analysis.json`；派生完整视图另保存在repo外 `complete-paired-summary.json/.csv`。

|shape|write Δ median|write Δ 五对范围 ms|endpoint selfRSS Δ median|RSS Δ 五对范围 KiB|
|---|---:|---:|---:|---:|
|save_fixed / unheld|-0.394%|-9.917..+10.971|-0.382%|-344..+112|
|save_fixed / held8|+5.366%|-8.603..+11.361|-15.407%|-4208..-3764|
|save_growing / unheld|-7.784%|-19.146..+9.796|-2.699%|-964..-256|
|save_growing / held8|-0.024%|-2.472..+14.297|-11.767%|-4412..-4080|
|rename_fixed / unheld|-3.743%|-14.902..+22.569|-2.617%|-648..-104|
|rename_fixed / held8|+18.455%|-43.033..+118.494|-17.340%|-5152..-4592|
|rename_growing / unheld|-2.274%|-6.774..+26.576|-3.601%|-1972..-680|
|rename_growing / held8|+2.393%|-13.552..+37.379|-15.233%|-10560..-5380|
|auth / held8 / 0 sessions|-0.244%|-8.198..+2.496|-15.252%|-4244..-4092|
|auth / held8 / 64 sessions|+2.022%|-0.766..+8.156|-0.440%|-228..-124|

所有十个shape的last8写都有正负混合，不能以某个中位数宣称稳定写收益或稳定回退。fixed held：AB三对全慢、BA两对全快；rename-fixed unheld相反。rename-fixed held为−31.66%..+121.93%，极散值保留。没有为了获得显著结果重采样、删异常或改变阈值。

### 读/恢复门槛的实际审核

以下路径**五对、AB和BA均回退**；同时给绝对差，避免仅以百分比放大小耗时。

|路径|配对中位回退|五对绝对差|
|---|---:|---:|
|save_fixed/unheld file_history|+26.25%|+1.4325..+2.8995 µs|
|save_fixed/unheld graph|+30.42%|+4.6320..+6.3845 µs|
|save_growing/unheld list_tree_owned|+200.58%|+6.9110..+10.6280 µs|
|save_growing/unheld file_history|+27.31%|+0.2475..+0.4380 µs|
|save_growing/held8 file_history|+27.92%|+0.2605..+0.4215 µs|
|rename_fixed/unheld graph|+21.43%|+3.6005..+6.6545 µs|
|rename_fixed/held8 file_history|+20.10%|+1.9480..+3.0440 µs|
|rename_growing/unheld file_history|+49.76%|+0.3710..+0.7260 µs|
|rename_growing/held8 file_history|+55.08%|+0.4660..+0.9715 µs|
|rename_growing/held8 graph|+11.86%|+10.5850..+36.3480 µs|
|auth/held8/0 file_history|+14.19%|+0.8420..+4.9145 µs|
|auth/held8/64 file_history|+12.47%|+1.7980..+3.7915 µs|
|auth/held8/64 graph|+10.12%|+0.7960..+5.2095 µs|

读回退不是每条query都回退：rename-growing unheld graph五对更快，中位−9.35%。上表held rename-growing graph（4.07%..14.68%）及auth64 graph（2.41%..16.79%）虽然中位超过10%，离散跨阈值，不能说每对均>10%；强拒绝依据包括file_history和其它确实每对>10%的路径。auth0 owned-tree中位+20.41%但绝对仅20..86ns，需计时分辨率限定。SQLite使用原正常策略，**实际hit未观测**，不归因某缓存命中。
每case三次reopen均为checkpoint miss/hit/hit，配对路径相同；只首个属于这里的full recovery。恢复没有建立稳定收益；fixed unheld第4对首恢复baseline9.033607667s / candidate4.521241330s完整保留，不包装成稳定50%加速。rename-fixed full recovery也有大幅离散。replay、首个recovery后新写、warnings/read-only/Engine.check及成员读回全部通过，相关计时不混进warm写。

### 内存口径

endpoint **self current RSS**是进程resident，含独立canonical验证buffers、auth setup、allocator/runtime，不是净State，也不是held-window峰值。held验证buffer分别为1,566,144 /5,293,464 /2,183,428 /6,491,092 B，auth0/64为1,573,560 /1,630,896 B；这些不是候选容器节省。self VmHWM是整个post-exec进程高水位；outer wait4 maxRSS可能有fork继承底线，始终单列，未用于内存采用结论。
约4–6MiB held endpoint下降在若干shape可重复，但auth64仅124..228KiB；rename-growing某对10,560KiB大差也保留。auth0 endpoint降约4MiB而self VmHWM差仍为−492..+640KiB，说明endpoint与全进程peak不能互换。未测allocations、syscalls、节点数、复制key字节或内部phaseCPU；whole-processCPU包含fixture/验证/recovery，不能归因write。

## 4. 事后补充：原先记录、未列入18项主配对分析的phase

以下是对既有raw的**事后描述统计**，不添加新gate或样本，不改变上面的不采用决定。原设计已记录全部三类cursor及snapshot/canonical/drop；此处不挑选有利项。完整相关phase在repo外 `complete-recorded-phases.json`。

|shape|event initial页 Δ median|tail-one Δ median|empty Δ median|snapshot acquisition Δ median（held8）|
|---|---:|---:|---:|---:|
|save_fixed/unheld|+2.34%|+32.42%|+33.33%|N/A|
|save_fixed/held8|+18.70%|+46.67%|+33.33%|-97.73%|
|save_growing/unheld|+13.23%|+83.62%|+74.39%|N/A|
|save_growing/held8|+21.98%|+82.28%|+72.05%|-99.20%|
|rename_fixed/unheld|+8.12%|+54.23%|+55.56%|N/A|
|rename_fixed/held8|+46.69%|+50.00%|+44.44%|-98.83%|
|rename_growing/unheld|+42.33%|+76.34%|+81.00%|N/A|
|rename_growing/held8|+59.29%|+91.24%|+81.82%|-99.01%|
|auth/held8/0|+53.13%|+75.00%|+72.78%|-99.05%|
|auth/held8/64|+54.12%|+82.27%|+76.25%|-99.13%|

Snapshot acquisition确实显著缩短（五对约104–194µs绝对下降），drop也单独记录，但它们在Engine写计时之外，不能冒充端到端收益。event initial页在若干shape每对慢约0.4–3.1µs；tail/empty差多为20–136ns，保留clock granularity限定。这批只有65/66/130events，完整分页oracle通过不等于>1000-event性能证明。canonical构造、旧snapshot验证和buffer释放另分栏，未从write时延扣减或以其CPU归因。

## 5. 完整性、预算、原始归档

五个formal jobs均attempt1成功、各48远程schema tests真实执行，各4 admission+20 formal进程只有1个真实passing test和1个成功marker。严格解析校验真实counts/operation receipts/events、全成员/版本readback、新graph/export、旧snapshot隔离、replay/recovery及first-write结果；两臂实际binary SHA、source/编译配置、overlay byte恢复和独立target身份闭环。全部18项×10shape×5pair从raw重新计算与远程analysis完全一致。独立复核完成记录见文末。
保留旧attempt 40493528598（编译失败、0完整样本）、40495245924（fixture私有权限约束失败、0完整样本）以及40499249383（旧pair两casepilot）全部原始证据，未拿重试替换失败。R48这140个进程没有失败样本筛选或自动重跑。

|suite|双臂构建 s|新admission s|formal阶段 s|完整ZIP B|完整raw B|
|---|---:|---:|---:|---:|---:|
|save_fixed|538.1|61.0|315.5|214,805,214|794,125,621|
|save_growing|636.1|59.4|293.5|246,399,219|896,973,809|
|rename_fixed|580.5|194.7|1026.9|319,141,256|1,101,868,605|
|rename_growing|720.9|284.3|1412.3|351,519,512|1,206,767,886|
|auth|687.7|78.0|390.5|216,841,333|799,321,337|

最长driver40m27s，保持原50min授权/55min step；各job原2GiB evidence与独立targets预算未扩大。标准hosted runner并非物理静默；fresh-process不是cold OS cache，未drop系统cache。五份formal ZIP合计1,348,706,534 B，包含4,799,057,258 B原始内容、735,091文件；735,086 inventory条目实际bytes/SHA全匹配（另5个inventory文件由ZIP digest绑定）。连同先验pilot，140进程全部ZIP及两臂binary保留；只提取必要JSON/log，不机械展开所有fixture。artifact七日过期前已本地保全，不把全部raw或生成的认证材料入库。

### 原ZIP SHA256索引

完整artifact ID/bytes/expiry/源head在旁附 [原始归档索引](persistent-state-r48-raw-archive-index.json)；此处列最小可复核索引。

三个实验refs已归档退役，source/raw/git bundle保留；分支清理没有删除run或原始证据。GitHub artifact仅七日保留，过期后须使用已保全ZIP副本，公开run链接不承诺artifact永久下载。

|run|阶段|ZIP SHA256|
|---|---|---|
|40520958337|先验pilot|`04c9058778ca40298bf4b100b7312224a084f31b46c99be1249c7751da405af8`|
|40521874910|先验pilot|`45371d85fd0e87287206d69929308c32f07b832809a4d22ffd74abb1788fc956`|
|40522175300|先验pilot|`7dfeda7f5820c9c342643f21f7cb35c4dc13944a614e242ef884fd80dd8c6d7f`|
|40522336457|先验pilot|`ce83c703cd209202b18a9a7edfa38f524aff85924505961527a823399d3bcaf3`|
|40522482612|先验pilot|`b168ac2c2e6941177d24f54f361388e7246192283dc08119960dac33afd7ee0a`|
|40526872615|4 admission +20 formal|`e672135f21098d915c95971a9ea8f12778f60d0dd5be9581806d0df6b8213ab3`|
|40527132872|4 admission +20 formal|`9712c9ea97e2ddf823278691d117bffecb416a88d39676a30c9926bea307b619`|
|40527348756|4 admission +20 formal|`bcdd94294da387e09a13fb0a46e4407a2f93ee4d0ba6f8b8a816afc14346801a`|
|40527607227|4 admission +20 formal|`cf64ae9bc2f2038e9ef237737a80c78f1d3f82aa5bfb5f84d66ef8ebe7bf98cc`|
|40527917679|4 admission +20 formal|`ef9a9377b1a44b9b1ece95bc08f8164330470e7b444ddd51c3b0800ed02b43c5`|

## 6. 复核与复现入口（不再派发实验）

精确[执行workflow](https://github.com/isarmg/zxs/blob/14354fbc2822a986415c4473010773a6e39e1d87/.github/workflows/ci.yml)、[comparison/receipts](https://github.com/isarmg/zxs/blob/14354fbc2822a986415c4473010773a6e39e1d87/test/measurements/persistent-state/comparison.json)和[driver](https://github.com/isarmg/zxs/blob/14354fbc2822a986415c4473010773a6e39e1d87/test/measurements/persistent-state/ci_entry.py)冻结了完整操作步骤。harness位于root `test/measurements/persistent-state/`，gate SHA256 `b77a4175195080b7cf51cd1c6adf4df485ccf89ef88158f4ac700af2103e6082`，comparison SHA256 `ebee90830ca5ba8332325479e7baa25bca03644e1b528e7ed5276a48f3c3e55a`。

只下载/核验/重算既有raw的示例（无需Cargo、不执行benchmark；原ZIP只读保存，派生目录另建）：

```sh
# 示例：save_fixed正式artifact；用已授权GitHub CLI访问，不输出凭据。
gh api repos/isarmg/zxs/actions/artifacts/12745197245/zip > save-fixed-raw.zip
printf '%s  %s\n' e672135f21098d915c95971a9ea8f12778f60d0dd5be9581806d0df6b8213ab3 save-fixed-raw.zip | sha256sum --check
# 仅在派生目录解出formal JSON；原ZIP及inventory继续保留。
mkdir reanalysis && unzip -q save-fixed-raw.zip 'formal/*.json' -d reanalysis
git show 14354fbc2822a986415c4473010773a6e39e1d87:test/measurements/persistent-state/gate.py > reanalysis/gate.py
python3 -B reanalysis/gate.py analyze reanalysis/formal
```

需要该精确commit在本地可读；每个artifact按索引核ZIP digest及inventory bytes/SHA，然后用同一gate解析。`analyze`只统计已记录JSON并写派生analysis，不进行构建/测试/Engine运行。复现实验执行入口是已冻结workflow的suite/stage选择，但本轮实验已结束；新的执行需要独立确定范围、预算与准入条件。

## 7. 决策解释与结论上限

私有Arc/AVL提供共享根、选定key的O(log N)路径复制；真实clone减少已观测。但有序扫描、反序列化/canonical输出、bulk mutable APIs、cache locality/Arc/node成本和必须的publication I/O仍在。candidate未新增unsafe/外部依赖/持久化格式，维护性不是简单按代码行数否决；需要真实端到端收益足以承担新的平衡/rank/迭代不变量。当前可重复读回退、写收益未建立及受setup影响的resident结果，尚不满足预设采用门。
因此保留已知正确baseline和全部研究证据，不把候选合入main；不再移动基线或增加样本追求通过。这个结论仅适用于精确full/omission-disabled R47/R48、64-save/64-rename开发三目录screen，不是生产吞吐、100k容量/渐近复杂度、大规模peak RSS、物理多盘、掉电或omission-enabled收益结论。

独立复核完成记录：2026-10-27 15:57 UTC五suite全部逐raw闭环；100 formal +20本job admission有效，全部18项×10shape×5pair及事后phase描述值与raw相符。`read_guardrail_review_flag`只是定位待审路径，不把“median>10且五对正”解释为统计显著或已证明超过离散；本决定综合明确逐对超阈路径、绝对差、顺序效应及未建立的端到端收益。
