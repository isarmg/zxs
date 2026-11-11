# 按原删除操作原子恢复文件批次

## 决策

整批恢复以已保留的 `file.delete.batch` 操作 ID 为唯一成员来源。请求另带本次撤销的永久 operation ID；调用方不能替换、遗漏或追加文件 D。预览读取当前全部原 D，提交在线性化点重新核对原批次及各文件当前所有者。

所有原 D 仍精确活动且满足普通撤销准入时，以一笔 `file.delete.batch.undo` 发布移除全部 D。任一成员已单独恢复、重新删除、路径所有者变化或被活动目录删除引用时，整批拒绝。文件正文、历史分支、目录工作树和旧永久收据保留。

提交采用现有发布协议。冷恢复按同一原批次的完整成员重放，FILE 审计保存整批撤销与每个原 D 的关联。当前尚未支持该审计映射的外来导入继续在原边界拒绝。

## 接口与类型

领域请求使用两个 OperationId 和已解析的 BatchUndoActor。完整 RestorableBatch 承载一次准入后的成员，业务失败使用 BatchUndoError；规范持久收据的编码与解码集中在 undo_record。

`GET /api/file-deletion-batches/{operation}` 返回原批次完整预览；`POST /api/file-deletion-batches/undo` 只接受 `deletion_operation_id` 与本次 `operation_id`。两个入口使用当前管理员授权；预览沿用有界响应 owner。Web 审阅完整成员后明确提交；发送前保存原意图，未知结果只查询同一永久收据，刷新后恢复同一请求。准确结果清理后才解除原操作围栏。

## 成本

单批最多 100 个文件，沿用总 frontier 和 512 KiB 响应预算。逐文件复用现有当前作用域检查；整个批次只有一次原生发布，没有新增正文写入或独立同步阶段。
