# 备份对象数据库只读快照

## 范围与依据

`PreparedDatabases::capture` 的输入只有一份已经准入的不可变 State，原实现仍构造同 State 的 `ProjectionTransition`，但只使用 `previous()`。这会在原 owner 索引外另外克隆或重建未使用的 next 索引，并建立第二份未使用的 move receipt 视图。

改用现有 `ProjectionSnapshot::new(state).view()`，继续以同一 `DerivedOwners::from_state` 和 `MoveReceipts::new` 生成所需来源视图。它不引入 authority、缓存或跨代际复用；原来的完整集合数量／字节预算、取消检查、文件唯一历史归属、复制生命周期缺口拒绝、SQLite 准入和精确字节核对全部保留。目录数据库仍使用原来的 scoped encoder。

仅删除一次备份捕获中不用的状态转换与额外索引，不改变路径、格式、请求、收据、授权、持久化屏障或公开 API。构建单份索引仍随 State 规模增长；未测量时间或峰值内存，不把静态删除的 clone 等同于性能结果。

## 验证范围

新增回归使用临时真实 Engine 创建文件容器和现有目录数据库夹具，对原始及下一序号的已准入快照，比较 FILE／DIRECTORY 全部 SQLite 字节与原 transition-based 投影结果。该测试保留原方法作为字节契约参考，不将 transition 重新带回生产捕获。

`backup-database-snapshot` 定向组选择全部原对象数据库安装／验证回归、新字节对比回归、一项完整 Backup 切换／后继冷重放，以及独立产品库编译。未本地编译或测试；CI 是否完成、具体数量和失败范围，以准确提交执行证据为准。

本片不新增普通目录功能，不接通新三目录布局、盘名＋UUID或改名；既有目录数据库仅作为同一备份完整性的已支持输入保留。系统全局、盘根本盘与容器本对象仍是三种不同范围；其完整代际恢复／迁移闭环仍待完成，不以此次对象投影字节相等决定任何新权威。

## 既有完整切换用例的超时定位

先前预算修复的 GitHub #1268（`ec55d63`）已通过 19 个 Rust 单测和 7 个 Python 模块共 116 项，但既有 `same_backup_switch_serves_and_replays_normal_successor_publications` 达到 900 秒时限；产品库检查未执行。原日志仅证明该测试未完成，没有给出卡点或根因，不能宣称完整 Backup 切换通过。

本次保留同一集成测试、断言和 900 秒预算，增加测试内部直接 stderr 阶段标记及只读进度观察。观察器仅用于该临时测试，每 2 秒读取至多 64 KiB 的已有 progress 文件，只输出变化，退出通过断开通道并 join 停止；不创建或修复任何生产文件。产品检查放在该集成前，以便超时仍保留其独立结果。此次索引精简不是该超时的已确认修复，后续修复须以新增阶段证据定位。
