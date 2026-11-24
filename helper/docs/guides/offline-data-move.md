# 停机把容器移到另一块 DATA

用同版已安装的 Helper 和 ZXS，一次移动一个活动容器。需要停机；源和目标必须是不同的、已登记且已初始化的 DATA。
移动不会改盘登记或格式化磁盘。ZXS 在确认路由切换已经持久化后才清理源盘上该容器的副本。

## 正常操作

1. 停止 ZXS，等服务退出，再用原非 root 服务账号操作。当前登记的 SYSTEM、RECYCLE 和全部 DATA 都须按原路径挂载。
2. 找到容器的 `CID`，为本次移动选定并保存 `OP_ID`。两者均为非零的 32 个小写十六进制字符。
3. 给出容器当前所在 DATA 和目标 DATA 的原登记路径。末尾按登记顺序给出当前完整角色名册，不只列这两块盘。

```text
helper disk move CID OP_ID SOURCE_DATA TARGET_DATA SYSTEM RECYCLE DATA [DATA ...]
```

例如，当前只登记两块 DATA，顺序为 `/mnt/data-a`、`/mnt/data-b`，并已设置 `CID` 和 `OP_ID`：

```sh
helper disk move "$CID" "$OP_ID" /mnt/data-a /mnt/data-b \
  /mnt/system /mnt/recycle /mnt/data-a /mnt/data-b
```

返回 `moved container` 且命令以 0 退出后，记下完成回执，再按原方式启动服务。容器号、版本、分支和原业务回执不变。

## 未确认或同号重查

命令未确认时，保留两盘和暂存，勿手工清理、换盘或换号；保持停机，用同一 `CID`、`OP_ID` 和原来的源盘、目标盘路径拼写续作。
历史重查也列当前完整名册；原来的源盘和目标盘无需仍在名册中。
`historical move completion` 只确认同号操作曾完成，不核对容器现在的位置。

见[验收范围](../../TASKS.md#安装后的普通迁移)与[恢复边界](../specs/offline-data-move.md)。
