# 非投票数据盘与混合新增实验批次

本批在重复纯添加协议上增加非投票新盘准入。源码、故障测试已编写，尚未编译或执行本批测试；本地只做源码格式与差异整理，正确性待独立 GitHub 验证。不得将前批通过结果外推本批，也不能据此合入 main 或开放管理 API/Web。

## 权威与资源边界

- `PendingMembership.members` 仅持有目标 metadata voters。新增 data-only 设备单独持有描述符、写锁和类型化 readiness，成功激活或同进程续作都转移其资源，不会因不投票而丢弃锁。
- 同一个不可变 intent 绑定全部新增设备、原/目标配置和历史边界。相同 voters 的纯 data-only 添加仍建立新 fence、归档、joint certificate、活动日志 namespace、authority 和永久结果，epoch 与 topology 各自检查递增。
- 新 data-only 根的 `data-ready.json` 绑定精确 pending seal、物理身份、transition、target config 与归档根。全体新增 seal 先落盘，随后同步 ready 文件及其目录；joint 前和完成观察前复验。只写元数据、不复制正文，不声称已有副本。
- 含非投票新增盘的 proposal/certificate 使用 `fenced-device-decision`，新增设备依赖通过 `metadata_map` 分页绑定。控制归档、实际 control/joint 日志和票数仍只属于 metadata voters。nonvoter 的 ready 不能当作旧票、新票或恢复多数。
- 已有 `fenced-joint-decision` 对象继续保持原哈希表示；该形态只接受没有新增 nonvoter 的过渡。新形态必须携带完整、非空、准确新增 data-only 集合。缺依赖或物理身份冲突拒绝，不能借旧形态漏掉数据盘准入。
- 激活逐一核对新设备与证书中的身份，再发布普通活动 seal。后续代际沿独立权威链定位该盘最初的准入证明。普通启动只读验证，缺失或损坏的 ready 不自动重建；显式 joint 验证也不补写 ready。
- 新 data-only 盘只安装最小 authority、配置锚和自身 ready，不安装全池控制归档或原正文的历史证明。其成为既存数据盘后，下一代按旧正文需要保留冻结 inclusion proof。历史 Candidate、原 proof、永久结果保持原字节/含义。

## 待 GitHub 执行的定向验收

新增九组 `membership_activation_data_` 测试，由现有 membership workflow 的激活过滤执行：

1. 纯 data-only 后再混合新增：固定 voters 2→2→3、Q=2、epoch 1→2→3、全部根锁保留、原永久结果不变；在两个新 primary 实际写正文，随后仅保留数据盘独立导出。缺 ready 或 portable data dependency 页明确失败；旧本地 proof 原字节不变。
2. ready 缺失时不产生 joint 观察，ready 损坏时不得激活或修改坏盘；恢复准确材料后同一过渡继续。
3. ready 同步后中断，冷启动仍为原 fenced 池，显式入籍同一根后完成，锁不丢失。
4. 纯 data-only 与混合各覆盖 marker、seal、Prepare、Accept、Finalize、Publication、配置投影前七个激活断点；同 authority、同操作恢复后实际写入新盘。
5. 丢失系统、移动 metadata 根后从固定 metadata 多数重建完整登记；data-only 根不能代替投票成员。

   GitHub 首轮暴露了真实恢复缺口：保留 workspace 的绝对 `source_path` 在根合法移位后被新路径前缀检查拒绝。修正从已认证代际链缓存准确 disk/slot/binding 的历史根名，仅将同 workspace 内的相对地址投影到当前已持有资源；不打开历史根、不改原 manifest/hash。原移位恢复覆盖保留，并补原 manifest 字节不变、再次冷开、未知地址、父遍历、错 binding 与跨盘主目录反例。此修正待下一轮 GitHub 执行，未作本地运行。
6. 复制 pending 根并改写 seal 的 device/inode，不能替换证书已经绑定的物理身份，拒绝且保留该盘字节。
7. 实际新增第三层 data-only 备份盘：只为后续正文建立第三份副本；原正文不被虚构为已有新增副本，其他盘离线后新副本独立导出。


8. 首次 ready 写入在 seal 已成立、正式文件尚不存在时留下合法临时文件：完整 canonical bytes、严格部分前缀和零字节 staging 均可同 op 冷重开；重新原子发布完整 ready，原临时文件保留不改。
9. 临时字节错误、属于异 transition、正式 ready 损坏，或正式文件正确但另有错误临时材料：续作拒绝，不改现存文件，不产生 joint 观察。

临时文件恢复只限已经验证的本 op pending seal 与其私有 namespace，文件名须是正常原子写暂存形态，字节须为由可信依赖重新计算的 canonical ready 完整字节或前缀。临时字节不提供权威，缺少原 seal、未知文件名或不同内容不能据此接管。测试模拟这些准确落盘状态，不声称物理断电验收。

继续运行既有激活、membership、recovery、proof、history、scrub、identity 等受影响过滤。没有新增完整 CI，没有本地构建、测试或真实设备操作。

## 未覆盖范围

不含下线、删除、正文迁移、设备返回、权限或 HTTP/Web。只允许纯新增，既存盘角色、绑定、路径、配置设置保持不变。持久未知材料仍不能无绑定接管；临时目录与故障点不代表物理掉电、跨主机 writer 或恶意特权克隆安全证明。

便携 authority 延续已有单对象上限预检，控制归档验证成本未作性能承诺。ready 与证书只证明本批指定准入/发布条件，不代替永久 I/O 撤销机制或后续下线协议。
