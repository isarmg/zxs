# 目录分支改名 HTTP 退役后的保留验证

原目录分支 Web 写控制与专属原请求重放已退役，目录标签 PATCH 的路由和 HTTP 解码／执行链也已删除，见[HTTP 写入口退役](../design/directory-rename-http-retirement.md)。此页不再描述可以发起目录分支改名的现行 Web／HTTP 操作。

## 保留行为

- 共享 namespace 恢复继续核对账户、会话、原操作、摘要／回执关联、未知结果、预算、冲突与失败清理。文件容器分支改名和节点操作保留。
- 不认识的旧浏览器记录保持原字节与围栏，不重放成新请求或静默擦除。既存服务端原操作继续查询。
- 当前目录标签与历史读取保留。目录 Backend 类型、适配器和 Engine 业务暂未删除，原标签／名称政策、永久收据、重放和撤权测试仍保留。
- 退役 PATCH 返回统一 404，不产生新操作或改变当前目录读取结果。

## 定向验证

`directory-rename-recovery` 保留共享恢复、mutation、inline 与 operation 原生测试及独立 Web 产品/WASM 编译；服务器 exact 由已删除的 rename decoder 改为仍存活的 default selection decoder，原 EngineBackend 标签、摘要、碰撞和永久重放回归继续选择。

`directory-rename-http-retirement` 选择真实 HTTP 退役与无副作用、文件分支改名所在行为，以及仍存活的队列取消和路由边界，独立编译 core／非 core 产品。原重叠组的预算和超时保持原值。

本地未编译或运行测试。以上是待 GitHub 执行的选择，不是通过声明。完整冷恢复、真实浏览器、SMB 与 Backend 整链删除尚未完成。

目录改名选择回归的四个 Python 测试已完整并入 `test_targeted_ci.py`，旧专用测试模块删除；原 rename 组选择该共享模块，不删除断言或放宽构建／模块预算。既有目录 default 集成场景同步改为验证退役请求拒绝、无改名审计和冷重开保留原标签。
