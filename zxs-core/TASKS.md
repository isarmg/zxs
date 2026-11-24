# zxs-core 验收状态

本页按功能记录证据，不把源码检查、托管 CI、实际文件系统和整套发行混在一起。[上手](README.md) · [架构](ARCHITECTURE.md)。

## 已核对的范围

| 能力 | 状态与证据 |
| --- | --- |
| Linux 库与 CLI 源码 | [仅静态] 独立源码目录、Rust 1.100.0、锁未改。格式与全目标严格 Clippy 通过；4 个纯内存 R3/地址单测通过，不是远端网络验收。 |
| 普通文件边界和无覆盖 | [实机已验] Dot Linux 临时 ext4、普通用户、有效能力为零。[3 个原生用例](src/native_tests.rs)与[工作区用例](src/workspace_tests.rs)覆盖租约竞争、拒绝不合格对象、晚占名和换件后不发布、不推进原操作。 |
| 确定性离线 ZIP | [实机已验] 同一环境跑[实际编译出的 CLI 用例](tests/linux_zip.rs)。两棵源树的创建顺序、权限、mtime 和被忽略文件各异，产生逐字节相同的 625 B ZIP；独立 unzip/Python 读回、晚占名、旧核对拒绝及准确差异清单通过。 |
| 独立 GitHub Actions（`9a74cd2`） | [CI通过] [Ubuntu 24.04 托管作业](https://github.com/isarmg/zxs/actions/runs/45335009882/job/138244922987)通过 Rust 1.100.0 锁定依赖、格式、全目标严格 Clippy、4 个纯内存及 5 个离线 ext4 用例；独立 ZIP 读回一致。未连接真实服务或安装产品。 |
| 显式原回执会话 | [CI通过] [`40c92bc` Linux 作业](https://github.com/isarmg/zxs/actions/runs/45337095000/job/138251648247)通过了[回执专用模式](docs/guides/receipt-only-session.md)新增的 4 个纯内存会话用例与 1 个 CLI 选择用例。安装后的真实 HTTPS/2 和冷恢复未验。 |
| 显式原节点区间 | [仅静态] [区间单独入口](docs/guides/payload-slice.md)只读已确认节点，每次最多 8 MiB。要求精确区间、原整份 ETag 与完整片段，只发布到新名；未连接真实服务。 |
| Linux 桌面输入与选文件 | [CI通过] [桌面作业](https://github.com/isarmg/zxs/actions/runs/45384195247/job/138410733529)通过 15 个用例。隔离 Linux X11 窗口实测中文显示、拼音、复制粘贴、回车和切窗保留草稿；Zenity 选择中文文件和取消均通过。 |
| Linux 桌面首次上传 | 无网卡 Ubuntu/ext4 客机通过回环 HTTPS/2 登录。预览不上传；一次提交发送一份完整文件，回执和读回内容一致。丢回执后重开只查询原号，未重传。 |
| Linux X11 重开后发送整文件 | 隔离 Ubuntu/ext4 X11 实测：[主线记录](https://github.com/isarmg/zxs/commit/a5fcb95ac591398e7566bf658a885079e336756f) · [实测源码 `658da86`](https://github.com/isarmg/zxs/commit/658da86cfcbe868e4f391300718bff1a46b55887)。重开、登录并确认后，原封存 `E1` 只发送一次，后来编辑的本机内容 `E2` 不变。 |
| Linux X11 离线丢弃封存 | 同一隔离客机。只丢弃选定的未尝试封存，未建立 TCP 连接，工作文件和原映射不变。 |
| Linux X11 冲突归档与重新提交 | 同一隔离客机。旧基线提交实际返回 `409`；离线归档同一原号 `O`，不联网。另存已核定的当前头至新路径，显式提交生成新节点 `N3`；旧工作文件、封存和映射保留。 |
| Linux CLI 单文件断点上传 | [隔离 Ubuntu/ext4 客机实测：`d75a1fe` 与 `cbe375c`](../zxs/docs/guides/resumable-upload-installed-qualification.md#已核对的-cli-客机验收)。覆盖真实回环 HTTPS/2 的断线、首块确认答复丢失后的显式续传及默认整文件对照。 |
| Linux X11 桌面断点续传 | [`6a4b51d` 隔离 Ubuntu/ext4 客机实测](docs/guides/resumable-upload.md#x11-客机验收)。收到首块确认后断线，冷重开和核对原回执未自动发送；显式继续只发末尾 137 字节，原回执、唯一节点和正文准确。 |

归档结果区只在登录后显示原号 `O` 的回执查询入口；本次实测只确认入口出现，未执行查询。

本次标准 ZIP SHA-256 为 `6f24562e993621528ce86967a40fa3ef4848d09332a37c7531818e4b4dcc9642`。证据和锁定命令见[复现实例](docs/guides/linux-ext4-qualification.md)。未验 XFS、安装包或真实服务器。

## 还要证明什么

| 能力 | 下一关 |
| --- | --- |
| 真实 HTTPS/2 与安装后的冷恢复 | [仅静态] 仅含本项目的源码独立安装。对真实隔离服务完成签出、原子编辑、完整提交和读回；服务器耐久写入后丢回复，重开只查原回执。留存第二次本机编辑且仍显示未提交。 |
| 错误与待决正文 | [仅静态] 在真实服务和 TLS 前端区分错账号、非 H2、空 503、错误 404、假回执；原正文丢失仍可查，但不发。控制组确认未提交时也不自动上传。 |
| Linux 桌面其他环境 | Wayland、密码页输入法组合、登录后的切窗仍待测。 |
| 分块传输待验边界 | [一个合法 8 MiB 流式上传](../zxs/docs/guides/resumable-upload-installed-qualification.md#已核对的单块流式上传)已在隔离客机通过。[仅静态] 仍待验[超时拒绝和截断](../zxs/docs/guides/resumable-upload-installed-qualification.md#流式链路待验)、会话到期、服务端重启和大文件吞吐。 |
| 归档原号的迟到回执 | [仅静态] [离线确认与恢复](docs/guides/conflict-archive.md)。另验按归档原号实际查询，确认迟到的精确回执只记入归档，不修改当前基线。 |
| 新接口与人机壳 | [设计中] 多区间的 ZIP/媒体读取、ZIP 远端差异、历史分叉 CLI 与其他平台壳。 |
| 交付 | [设计中] 核对准确许可和版本库存、独立安装/签名/各端运行。未删除旧路径或旧历史。 |
