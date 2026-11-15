# 同次 FILE 多副本材料复用

基线：`5d0ffffe6a1ea63ad1c1bf2e39995a0c087891af`。本切片不引入原件归属、原件清单、延迟交付或新持久格式；FILE、归档、请求摘要和数据库安装 envelope 的字节协议均不变。

## 实际重复与消费者

`container_storage::prepare_names_inner` 对同一 FileId 的实际可用物理位置，原先各自捕获同一完整逻辑行、计算摘要并编码 SQLite。名称描述的按 history 缓存不是这份 FILE 材料，不能代替它。

本切片仅在第一个实际容器通过原实体准入后，构造私有 `PreparedFileCopies`。它借用原 `StateProjection`，同时保留该投影第一次编码的不可变 image。后续目标没有接收替换 State/view 的入口。每个目标仍独立捕获根下容器、目录身份和旧 FILE，再交原 preserving installer；没有复制前一目标的物理观察或安装授权。

`CapturedFile` 的已有单次 owned capture/prepare 保留独立用途：只读未发布安装对读取仍在原位置执行，不得到共享缓存或新的写权限。它的“捕获后改变调用方 State 仍消费旧材料”回归保持不变。

## 顺序、范围和预算

- 每文件缓存只在原 slot/history 顺序遇到第一个实际容器时建立。历史恢复缺失资源、被排除根、缺失 history/container、历史 shared-layout 跳过规则全部保留，不提前解析或编码被跳过材料。
- 第一份仍按 entity → inode → 完整行准入/摘要 → SQLite encode → scope → 旧 FILE/安装对准入的顺序。
- 后续份仍先捕获各自 entity/inode；来自 previous 与 next 的 location 若 history 不同，仍在原逻辑准入位置拒绝，不把缓存 history 偷换给目标。
- 后续份省去的只有已由同一不可变 view 证明的行捕获和编码。旧 FILE 格式/范围、保留安装对、实体替换、逐目标 reservation 检查未删。
- 每目标旧 FILE 读取仍先于原 64 MiB 总安装字节检查。实际复制份数逐份计费；共享内存不把磁盘安装费用折成一份。描述 cache spool、版本改名、库存字段和最终 reservation 检查没有改序。
- 公共 installer 只接受可拥有的不可变共享字节。其他 DISK、目录和备份对象消费者仍传原 Vec，按原相位转换为 Arc；没有新的持久标记或权限模式。普通 FILE/归档需 Vec 的既有边界从唯一 Arc 取回，不复制第二份池级 image。

## 证据与限制

新增真实三副本发布和冷开，逐份和原 Schema 编码 oracle 比较完整字节，核 BODY inode/字节不变；实际 name preparation 仍计三份费用。共享 adapter 回归验证同一 image 被三个真实 target installer 持有，并对另一 history、第二目标损坏 FILE、捕获后容器替换保持拒绝；另一 State 的不同材料不能重绑已有共享对象。历史缺资源夹具证明即使逻辑材料越限，全部跳过的恢复仍不触发编码，普通新准备仍先报资源缺席。

已有 FILE codec、捕获后 State 变更、物理/逻辑双错优先级及 preserving install pending/completed/crash 对回归继续选中。新增窄组 `file-material-copy-reuse` 选中旧 FILE 18 项、新 adapter 3 项、installer 模块（保留 cap 16），并核默认和 lean 产品编译；不放宽旧组或 900 秒限制。

只完成静态调用图、签名、源码拼装和精确 hunk 核对。未本地编译或运行测试，未执行 selector；等待根线程对准确集成提交定向 CI。并非 native owner 或完整重构验收。
