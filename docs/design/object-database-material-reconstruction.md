# 对象内部数据库的独立救援归档与重建

[三范围数据模型](../07-data-model.md#s07-03) · [文件内部库](file-local-database.md) · [盘根材料](offline-disk-database-material.md) · [辅助工具](../components/helper.md)

## 目的与范围

这条链实际从文件内部 `.zxs-file.sqlite` 或目录仓库内部 `.zxs-directory.sqlite` 读取完整本对象记录，形成可单独保留的救援归档，并且能在**原 SYSTEM、原盘和原 SQLite 都不可用时**，仅凭该归档与明确对象身份重建一份新的独立 SQLite 镜像。

这不是安装或权威恢复：自摘要和本地结构闭合不证明原 publication、最新状态、正文可用或当前授权。结果不能直接接入活动文件容器、目录仓库、运行池或备份写者。退役节点不会因能重建其元数据就复活，子仓库/文件引用也不会因为在目录内出现就取得读取能力。

三个范围仍不同：SYSTEM 保存全局，盘根保存本盘及路径，对象内部保存本对象。归档不添加宿主目录地址、盘登记、外部 owner 路径或原路径事件。目录树里的路径是本仓库内部相对组成；自由文本备注/提交信息保留用户原文，不作内容删改。盘根独立材料入口保持原有范围，没有由本片升级成多数表决或完整系统恢复。

## 实际入口

Engine 库公开：

- `inspect_object_database_material(directory, path, expected_scope)`：读取操作者明确指定、已经持有的对象目录；对象可以是独立救援拷贝，不要求仍处于原盘目录树。选择 File 时必须提供 file/history 身份，选择 Directory 时必须提供 repository/directory 身份。
- `ObjectDatabaseMaterial::archive_bytes()`：生成严格 `object-local-database-material` 归档，包含作用域、原物化序号、完整有序记录与原逻辑摘要。记录正文保留规范 JSON 字节。
- `reconstruct_object_database_material(archive, expected_scope)`：不执行文件 I/O，在内存逐项重新准入、重新编码当前 SQLite 格式，并再次完整解码比对所有记录与作用域。返回独立镜像与其材料，不返回 installer、Engine、写者或新 State。

辅助工具提供三个显式 CLI 命令：

```sh
zxs-helper object-database inspect selected-object.json
zxs-helper object-database export selected-object.json rescued.object-db.json
zxs-helper object-database rebuild rescued.object-db.json selected-scope.json rescued.recovered.sqlite
```

文件选择示意：

```json
{
  "entity_directory": "/rescue/one-file-container",
  "expected_scope": {
    "kind": "file",
    "file_id": "file_11111111111111111111111111111111",
    "history_tree_id": "history_22222222222222222222222222222222"
  }
}
```

重建的 `selected-scope.json` 是上面 `expected_scope` 的对象本身。目录作用域使用 `kind:"directory"`、`repository_id` 和 `directory_id`，其中独立目录是 entry ID，仓库根则与 repository ID 相同。身份必须由操作者选择，不从待恢复字节自动猜测。

Inspect/Export 必须先停止全部相关写者，并持有当前 daemon 共用生命周期租约；旧二进制、Samba/其它主机或未参与租约的进程仍须人工停止。Rebuild 不打开原对象、SYSTEM 或任何盘，不需要创建系统目录或获得活动服务租约。

## 读取边界与数据保全

对象目录经过 held-FD 和具名祖先身份核对。SQLite 仅在内存打开；文件只经受限相对解析、有界读取的单链接普通文件 FD 进入。真实缺失、符号链接、硬链接、特殊文件、错误身份、损坏、未知 schema、读取错误和 `.sqlite-wal/-shm/-journal` 都明确拒绝，不按“没有数据”处理。

读完保留原 FD，再核同名文件 inode/大小/字节摘要、侧文件仍不存在及目录具名身份；不创建锁 sidecar、缓存、目录或修复文件。归档不附带当前目录地址，搬动救援材料不会偷偷给它新 authority。

辅助工具输入文件必须由当前操作者拥有，读取有界并不跟随叶链接。Export 仅写新的 `.object-db.json`，Rebuild 仅写新的 `.recovered.sqlite`，这些后缀排除活动托管数据库名称。写入复用已有 held-parent、0600 暂存、文件同步、无覆盖发布、目录同步；已有输出、链接或目录胜出，不覆盖。发布/目录同步结果不确定时保留实际输出/暂存和具体错误，不能把失败当成未写入后自动重发。

## 真实结构校验

文件材料复用现有完整 FILE row decoder：仅接受 file/version/branch/anchor/metadata/deletion/lifecycle/merge/merge_link；验证本文件与历史域、节点序号、真实版本声明、保留分支 HEAD、当前 D 与原 delete/undo 转移。读取新增按 typed 行重新编码的规范表示检查，未知路径字段不能靠重算自摘要通过。它仍不是完整跨对象历史证明，外部合并引用、原 publication 和全部正文需另行验证。

目录库共享读取边界解码 repository/branch/tree/commit 的闭合类型，普通安装前核验与离线救援使用同一套规则。仓库身份、主/当前分支、HEAD/index 存在、树所属分支、本地 parent 边和无环、原用户 commit/tree 对应关系均须成立；树外部 root path 必须为空，条目、上下文、备选版本与内部引用保持相对范围。相同文件的合法多路径 alias 保留，不把重复文件 Entry ID 误当另一个目录；目录或不同文件不得复用该身份。不能通过删去全部树、增加外部路径字段或重新散列非法正文得到“已恢复”。

独立子仓库与文件引用只保留原稳定 ID，不追随可变 HEAD，也不读取外部对象。普通工作树 `commit_id` 可以指向没有保存在本对象库的外层命名空间提交；本片不制造该提交、不把外部引用标作已验证。这些本地结构规则现已收敛到共享解码出口，普通布局准入、旧库替换前核验、隔离准备与救援不能再只靠自摘要接受非法记录；不重复维护第二套目录结构规则。实际边界、成本与未验证范围见[数据库恢复读取准入](database-recovery-admission.md)。

## 资源和未完成项

- SQLite 镜像仍最多 16 MiB，本对象记录 ID+正文合计最多 12 MiB、65,536 行。归档因 JSON 字符串转义与信封最多 32 MiB，完整输入先有界读取再解码，不接受分页缺失的“部分归档”。
- 索引读取/结构验证/编码与二次解码是 O(记录字节及有界关联) 加有序表开销。目录父边访问只处理有界本地集合；SQL 延用现有值、语句、VM 编译和 bind 限制。保存原始行、typed 目录结构、归档和 SQLite 镜像可能同时占内存，这不是总 RSS 上限。
- 归档与重建不改变原格式标识，不增加旧格式双读/升级。归档缺失、旧库不支持、未知侧文件或记录不完整时保留拒绝，不从版本文件名补猜缺失历史。
- 仍缺无归档情况下从各原生 publication 独立重建完整对象库、恢复后的跨范围联合准入、代际档案和正式活动库重新安装。仅有这份归档不能恢复全系统或宣称任意幸存盘已恢复全部数据。

2026-11-08：生产源码与七条回归源码已写入候选；按当前要求未审查、编译、格式化、静态检查、测试或 CI，未操作真实磁盘。源码中的删除只存在于未运行临时目录测试，不表示已经执行过失盘实验。

## 已发表镜像的准确读回

后继 [救援数据库读回](rescue-database-readback.md) 给重建结果增加完整语义 `verify_database_bytes`，helper 重建发表后绑定原 inode 冻结再读，只有实际文件与原归档完整记录一致才返回 `file_readback_verified:true`。新增 `object-database verify ARCHIVE SCOPE IMAGE.recovered.sqlite` 可在原进程/原 SYSTEM/原盘均不存在时只读复核；不修复缺失或错误输出。发表后失败保留新文件并明确未核验，不提升来源认证或活动安装权限。后继组合的生产 check、helper/平台/目录定向验证与未执行的 Engine libtest 范围见读回协议交付事实。
