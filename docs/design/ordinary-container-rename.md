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

进入原 ACCEPT／全员 FINALIZE 后，先写入既有 inode-bound version-name intent，
再把每个已验证历史 BODY 改为同一 `15位身份ID-新文件名`。完整正文哈希、大小、
inode、stamp 和租约沿用原唯一捕获核。随后移动外层同一个容器 inode，安装原 FILE
投影和 cache，核对并清除同决定的 intent，才进入原 Publication 尾部。
原 BODY 身份前缀、内容和容器归属均不改变。

## 冷回放不是用当前字符串改写旧记录

旧 Snapshot、Candidate、操作回执及 FILE 来源记录仍保留原字节。实际 held journals
的唯一 Published 游标只为已验证改名边保留只读 location projection，绑定完整原
owner、namespace 父项、容器 replica occurrence 及每个名字生效的 sequence。
历史读仍使用原 State 的权限、源链、父目录和 inode，只由该连续证据选择终态名称。
后来新增的副本不能替较早的另一副本供应证明。同旧名新建一个不同 FileId 不会被采用。

未有全员 FINALIZE 的 pending 候选只能观察旧名。接受过一个 ACCEPT、同 inode 被
外界移到新名、磁盘上存在 intent 都不允许恢复器收养新路径。真实扫描的全员
FINALIZE、各自前序 ACCEPT 与内容寻址对象完全绑定后，恢复器才可观察该决定的
旧或新两个位置；两者皆在或皆不在均拒绝。叶子只能由同 file/history/sequence/
source/target 的 intent 定位。该能力只进恢复消费者，不授普通公开读或新写权限。

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

## 定向证据（待准确提交上的 GitHub 执行）

- 两次不同 BODY 和非空标签：先 A 上公开改名，再由真实 automatic backfill 增加
  B/C 并冷开，之后第二次改名覆盖全部三个副本。原 inode／字节、准确重试和旧
  名字上独立新 FileId 均保留；另两用例从一开始就持有三个实际副本。
- B 上目标名碰撞：预算及实际 root/实体门仍先行，未发表、未推进 sequence／日志，
  不覆盖 sentinel，不改变旧正文或原容器。
- 真实七个 fault 点：`after_accept_1`、`after_finalize`、`version-names-intent`、
  `version-name-renamed`、`ordinary-container-renamed`、`after_container_names`、
  `after_publication_1`。核外层 InDoubt 与精确注入 cause，然后冷恢复与准确重试。
  ACCEPT-only 的外部同 inode 改名和 Publication 尾的旧名回退分别先拒绝；恢复原
  现场后才继续正控，不把损坏拒绝当成功恢复。`after_container_names` 另验证
  intent 已清但未 Published 时的坏 BODY 必须拒绝，恢复原 bytes 后再完成。

没有改变旧 CI 分组、cap、timeout、原备份切换 900 秒或已有历史固定向量。
尚未验收 Current／移动／删除、断盘改名、混合内容改名、真实 SMB 和浏览器交互。
