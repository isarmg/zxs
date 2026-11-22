# Linux 桌面：签出、改动、补回和提交的验收

用生成的三个小文件和一个空目录，从已安装的 Qt 窗口验证一条完整本机工作流。种子由正式 CLI 提交生成第 1 代；签出、补回、提交第 2 代和查询该笔回执都必须在**同一个原窗口**操作。源码中的 CLI 格式工作树关闭后仍可从原 STATE 查询；桌面冷重开既有 STATE 尚未提供。

## 1. 环境与种子

- 在经授权的可丢弃 **Ubuntu 26.10 amd64** 客户机上，按 [Ubuntu 配方](../../packaging/ubuntu-26.10/README.md)用同一份源码制作候选。仅安装同版的 `zip-workspace` 和 `zip-workspace-desktop`；保留源码提交、输入清单、两个安装包摘要和安装版本。使用普通账户以及真实的图形会话，不用 offscreen Qt。
- 另选现有、本人独占的 **0700** 暂存父目录，位于本机整个 ext4/XFS 挂载上，不用绑定子目录、overlay、SMB 或 FUSE。父链不含链接，不得由他人写入；关闭同步。操作者须保持这棵合成目录不被挪用或移挂载。准备和验证均不需要真实归档或现有工作树。
- 在 `zxs-workspace` 目录打开终端。把变量路径换成本次使用的全新位置，保持同一个终端：

```sh
RUN="/你的本机私有暂存目录/workspace-gui-01"
FIXTURE=tests/qualification/linux_desktop_workflow.py
/usr/bin/python3 -I -B "$FIXTURE" prepare "$RUN"
```

`prepare` 要求 `RUN` 不存在。它只创建这一处合成目录，用安装的 CLI 登记并提交 `RUN/seed`，输出准确的 `archive`、种子节点和整包 SHA-256。发生错误保留此目录；不要在同一路径重跑或删除材料。

## 2. 打开和签出

用刚才输出的绝对 `archive` 路径启动窗口，保留这个终端：

```sh
ARCHIVE="替换为 prepare 返回的 archive 绝对路径"
/usr/bin/zip-workspace-desktop "$ARCHIVE" &
```

1. 核对独立确认窗中的路径，点「只读打开并核验」。在 `A`、`B` 分别找到各自的 `report.txt`，并打开空目录 `empty`。
2. 在「工作树」点「签出此快照到新目录」。依次选择已存在的 `RUN/store`、`RUN/work`、`RUN/states`，新名称输入 `design`。核对原节点和两条新路径后确认。待窗口显示「原操作已签出」，记下「签出操作」的 32 位编号。
3. 窗口空闲时，用该签出编号核对。所有 `verify` 都只读已生成的合成文件和原 CLI 回执，不执行恢复或提交。

```sh
CHECKOUT="替换为窗口显示的签出操作编号"
WORK="$RUN/work/design"
/usr/bin/python3 -I -B "$FIXTURE" verify "$RUN" checked-out "$CHECKOUT"
```

结果应为第 1 代、无待决；签出回执指向原种子，工作文件、空目录与原包一致。保留该 JSON。

## 3. 编辑、对照、补回

只改刚核对的合成工作树。以下同长编辑便于区分实际内容比较与只比较文件大小；移出的合成文件保留在工作树外。

```sh
printf 'A\nafter!\n' > "$WORK/A/report.txt"
printf 'new local line\n' > "$WORK/A/local.txt"
mv -- "$WORK/missing/restore.txt" "$RUN/removed-fixture.txt"
/usr/bin/python3 -I -B "$FIXTURE" verify "$RUN" edited "$CHECKOUT"
```

1. 在原窗口点「查看工作目录相对基线的改动」，确认是第 1 代。应恰有三行：新增 `A/local.txt`、修改 `A/report.txt`、缺少 `missing/restore.txt`。不应有 `B/report.txt`。在下方完整相对路径栏核对选中项；可点「核验所选小文本的逐行差异」，看 `before` 变为 `after!`，关掉文本页。
2. 先关编辑器、停掉其他写入。选择 `missing/restore.txt`，点「从登记基线补回所选缺失文件」。核对原基线、缺失路径和要写的位置，点「把缺失文件补回原路径」。原报告随操作关闭，完成后重新打开当前基线对照。
3. 新报告应只剩一个新增和一个修改。窗口空闲时运行：

```sh
/usr/bin/python3 -I -B "$FIXTURE" verify "$RUN" restored "$CHECKOUT"
```

它检查文件补回的原字节、已有编辑、原种子 SHA-256、原第 1 代登记和 STORE 仍只有原槽。保留对照结果和这一阶段的 JSON。

## 4. 明确提交与核对

1. 在**重新生成的当前基线对照**点「提交本次审阅的整棵工作树」。先取消确认，重新运行上一条 `verify … restored …`，确认仍是第 1 代且无待决。重新打开当前基线对照，再点提交。核对 WORK、STATE、STORE、已审树摘要，记下**第二次**操作号。保持全部写入停止，勾选确认并点「确认并提交本次审阅」。不要通过 CLI 代提交。
2. 等待窗口显示第 2 代，点「查询该次操作的原回执」，应显示同号的「原操作已记为完成」。保留界面的实际操作号和结果；若返回不确定或失败，保留原路径、操作号及报错，停止本次验收。
3. 将窗口该笔完成回执里的操作号填入 `COMMIT`，运行：

```sh
COMMIT="替换为窗口显示的原提交操作编号"
/usr/bin/python3 -I -B "$FIXTURE" verify "$RUN" committed "$CHECKOUT" --commit-id "$COMMIT"
```

验证器要看到同仓同分支的第 2 代以原第 1 代为父，目录和正文与审阅后一致，正式包与原私有候选 SHA-256 相同，状态无待决且本地对照干净。原签出回执仍是原节点，无额外 STORE 文件。

最后点「关闭本窗口的工作树会话」，核对原目录与 STATE 后确认关闭；再次运行上一条 `verify`，结果应相同。保留四个阶段的 JSON、界面上的签出及提交回执、Qt 会话类型和实际候选版本。该流程尚待运行；本轮不挂载、不测共享写入。
