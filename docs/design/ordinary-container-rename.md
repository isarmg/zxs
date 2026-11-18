# 普通容器同父目录改名

本实现片以 `f70e2a49b02ff016bb40deaed2618ea583a479b9` 为前像。
它继续现行新格式，不新增旧池解释、迁移或双布局。本文是实现与验收范围，
不是本地或 GitHub 测试通过声明。

## 已接的决定与物理边界

首片限于实际 Initial 普通命名空间中的纯文件改名。原 repository、branch、
EntryId、父目录、FileId、历史身份、owner 与每个副本实体保持不变。
原公开 `file_namespace` 继续保留主体、只读、幂等及 namespace 前置门。
原永久 path_changes 与实际前后 namespace 必须吻合，且前后 VersionId 相同。
新 Candidate 的 file_containers 只反馈同一 record 的当前名称；版本、placement、
正文、文件分支 HEAD、文件节点和标签不因改名生成新内容历史。

不把 kind 字符串、任意 Config 或序列化 intent 当改名许可。普通规划和冷
Candidate 仍分别消费同一完整容器转换核。Current、跨父目录、整容器生命周期、
混合改名加 BODY 写入、有未提交 Draft 的改名均保留明确拒绝；这不是磁盘名传播。

所有目标先完成实际名称预算与原实体观察。新名已有任何文件、目录或链接都拒绝，
不交换、不删除、不按相同内容采用。实际旧父目录与容器 FD 持续保留；同父目录
NOREPLACE 使用原同步原语，保留同 inode 父目录的去重，不重定义耐久性顺序。

普通改名的决定只移动外层同一容器 inode，随后安装原 FILE 投影及 cache，进入原 Publication 尾部。
每个已安装 BODY 用自身 `VersionStorageId-creation_name` 校验：原父目录、原 inode、内容、大小及目标组件预算均保持；新建 BODY 才产生新叶。没有内部逐叶改名的步骤、恢复意图或兼容查找。只改非主文件分支名称、选中节点或复用节点均不能触发这个外层转换。

## 冷回放不是用当前字符串改写旧记录

旧 Snapshot、Candidate、操作回执及 FILE 来源记录仍保留原字节。实际 held journals
的唯一 Published 游标只为已验证改名边保留只读 location projection，绑定完整原
owner、namespace 父项、容器 replica occurrence 及每个名字生效的 sequence。
历史读仍使用原 State 的权限、源链、父目录和 inode，只由该连续证据选择终态名称。
后来新增的副本不能替较早的另一副本供应证明。同旧名新建一个不同 FileId 不会被采用。

未有全员 FINALIZE 的 pending 候选只能观察旧名。接受过一个 ACCEPT、同 inode 被
外界移到新名都不允许恢复器收养新路径。真实扫描的全员 FINALIZE、各自前序 ACCEPT 与内容寻址对象完全绑定后，恢复器才可观察该决定的旧或新两个外层位置；两者皆在或皆不在均拒绝。无论在哪一侧，每个应在的 BODY 始终只有原创建名的一个合法叶。该能力只进恢复消费者，不授普通公开读或新写权限。

已有 Publication 对象但缺部分收据时，必须绑定原完整结果及实际全员 FINALIZE
收据。该尾部只能接受目标容器名及应在该副本的全部 Available 历史 BODY，不能
先补 Publication 再容忍旧名称。更早、已完整 Published 的历史槽不追加这种
终态正文义务，保留原先对后来合法清理或丢失材料的历史处理。

连续性核只检查实际 file_containers 差异键中的已跟踪 FileId。独立解码没有可信
增量时仍遍历已跟踪项，但跳过整值未变项，不宣称冷回放 O(delta)。现有普通
ContainerPreparation／verify_transition 保留所有旧 replica occurrence，仅允许附加
新副本；降低 route copies 或 purge 正文不等于删除此实体归属。后来真正移除／
重指 occurrence 的迁移需要独立真实生产证明，本片不会替未来事务补权限。

历史路径修复不再用旧 shared-layout 给普通容器写回旧名；最终普通投影仍负责
当前 FILE／cache。没有有效连续证据时拒绝，不做全盘扫描或同内容搜索。
历史槽完全缺 journal Publication、只能由旧 local-proof 路径发现的情况，仍先走
原发现门；本片不绕过它，也不承诺能在终态名字尚未认证时修复该缺失证据。

## 定向验证目标（不代表已运行）

- A 名创建的旧 Version、主分支改为 B、B 名下新增 Version：三盘有效副本只移动各自的外层原目录；两个不同叶名持有原 ID、inode 与字节。独立文件分支改名或选择不更名该容器，不制造新 BODY。
- 外层目标碰撞或已有未知、被替换的 BODY：拒绝覆盖、收养或继续错误尾写；保留原实体。Pending 不借未经全员 FINALIZE 的外部改名恢复。
- 从原决定冷开、丢失回包重试，以及对应恢复、历史导出、受控 HTTP/管理读取均核相同的原版本。受控下载名与当时的存储名按[命名合同](../adr/container-current-name.md)分别判断。
- 此处不证明跨父目录移动／整容器删除、断盘改名、混合内容改名、真实 SMB、备份 900 秒或浏览器交互已经验收。
