# Windows 与 macOS：托管桌面验收

给维护 Workspace 预览和另存的人员。本页解释 [Workspace desktop qualification 工作流](../../../.github/workflows/workspace-desktop.yml)。只登记实际运行的提交、平台和步骤，不把合成用例当成发布。

## 托管范围

| 环境 | 观测与限制 |
| --- | --- |
| Windows Server x64 | [CI通过] [原生 Qt 窗口与真实本机 NTFS 的只读用例](https://github.com/isarmg/zxs/actions/runs/45322700879/job/138204775250)。Windows 11 普通用户权限、安装和文件关联另验。 |
| macOS 26 arm64 | 正式候选 [仅静态]；[这一轮原生窗口、APFS 和候选包作业未通过](https://github.com/isarmg/zxs/actions/runs/45322700879/job/138204775505)。运行配方只用 ad-hoc 签名，不是 Developer ID、公证、Finder 或 Gatekeeper。 |

流程在原生 Qt 后端试开选定 Store ZIP、查看空目录和精确同名路径、退出后查原件。Windows 另有不依赖驱动的只读数据面；macOS 配方分开记录源码、包摘要、解包签名与窗口启动。

托管环境不安装驱动、不挂盘、不变更系统安全设置，不读真实资料或上传候选二进制。具体钉住的镜像、Python 和 wheel 以[工作流](../../../.github/workflows/workspace-desktop.yml)为准。

## Windows 远程原节点另存

[仅静态] 托管 Windows 使用管理员环境。若实际令牌和原 NTFS 私处不符合要求，另存资格显示 `NOT RUN`；这不影响上面的只读证据，也不是写入通过。不为测试降低权限门槛。

在 Windows 11 非提权普通用户下，选本人所有、现有、独立的本机私用 NTFS 测试目录。运行入口（在 `zxs-workspace/` 中）：

```powershell
python -I -B -X utf8 tests/qualification/windows_node_qualification.py 'C:\路径\现有私用测试目录'
```

仅使用合成 H2 正文。系统必须实际返回原来源区标记，文件身份、ACL 和无覆盖由真实 NTFS 核对。共享、同步、EFS 和真实资料不适合作测试目录；不调整 ACL 或来源策略，不递归清理合成案例。

## macOS 原节点另存

[仅静态] 严格路线要真实读回原下载 URL，不因隔离类型存在便放行。[macOS 26 的真实作业](https://github.com/isarmg/zxs/actions/runs/45298114940/job/138123553971)中两种表示都未读回 URL。同一作业另有两个 `test_synthetic_provenance_*` 用例通过：错误摘要不发布、晚占名不覆盖。严格来源用例另因 URL 缺失而失败，不可用合成结果替代。

独立的显式另存路线用真实 APFS 和系统隔离；源码用例的同意和传输是合成输入，不能替代用户在前台签名应用中的真实同意。未读回 URL 时必须如实显示，发行入口仍关闭。

开放前用 macOS 26 arm64 前台候选验证长路径、Unicode、完整 URL 和 SHA-256 可读；Return、Escape、关闭或取消不批准，只有明确肯定才开始。换选、换父、占名不得写入替件。

相关：[FSKit 独立资格](../../macos-fskit/target/QUALIFICATION.md) · [旧版完整步骤和证据](https://github.com/isarmg/zxs/blob/85a8988c386baad1305f4f81ce2c57786df6e436/zxs-workspace/docs/hosted-desktop-qualification.md)。
