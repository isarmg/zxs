# 架构参考

[文档目录](../README.md) · [组件说明](../components/README.md)

这里解释当前实现及其边界，不重复维护改进计划、阶段总结或验收流水。行为规范见编号章节，工程规则见[第 20 章](../20-engineering.md)。

## 存储与界面

- [写入流程](write-flow.md)：规划、准备、决定、发布和失败边界。
- [工作区提交准备](detached-workspace-commits.md)与[写入准入](detached-write-admission.md)：引擎锁、冻结基线、取消和验证。
- [恢复检查点](verified-recovery-checkpoints.md)：权威证据、修复义务与回放条件。
- [快照与历史规模](history-growth-optimization.md)：分页、缓存与增长限制。
- [读取投影](read-projections.md)：请求上下文、复杂度与冻结响应。
- [文件列表投影](file-list-projections.md)：不可变快照、键控行与事件安全。
- [浏览器响应式运行时](reactive-browser-runtime.md)：CSR、客户端模板与生命周期。
- [G4 轮廓失效与布局批次](contour-invalidation.md)：触发源、失效集合、读写批次与清理。
- [组件与插槽属性](view-prop-parser.md)：选项语法与诊断。

## 基础能力

- [任务调度](task-dispatch.md) · [并发集合](concurrent-collections.md)
- [SQLite](sqlite-access.md) · [压缩](compression.md) · [Schema 验证](schema-validation.md)
- [数值](numeric.md) · [日期时间](date-time.md) · [UUID 语法](uuid-format-validation.md)
- [模式匹配](pattern-matching.md) · [Unicode 数据](unicode-data-families.md) · [URL 编码](url-encoding.md)
- [表单浮点格式](form-float-formatting.md) · [错误定义](error-definition-naming.md) · [终端样式](terminal-style.md)
- [原生 C 工具链](native-c-toolchain.md) · [C 表达式解析](binding-expression-parser.md)

HTTP、序列化、密码学和其他公共库见[组件目录](../components/README.md)。

## 工程结构

- [Foundation 职能与包边界](foundation-groups.md)
- [自动生成的消费者与特性索引](foundation-consumers.md)
- [源码身份与来源边界](source-naming.md)
- [测试布局](test-layout.md)
- [Foundation CI](foundation-ci.md)

原始上游文档、许可证及来源证明由集中合规材料管理，不能用本文替代或改写。
