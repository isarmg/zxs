# Samba 4.23.0 完整源码

`samba-4.23.0.tar.gz.part-*` 按顺序保存完整、未修改的官方 **源代码** 发布包，已实际纳入 Git，
不是编译产物、下载指针、Git LFS 指针或 submodule。压缩包保留上游目录、构建器、
所有许可证、版权头及自带公开测试夹具；其中示例测试密钥不是 ZXS 部署凭据，不得用于生产。
原包仅按 4 MiB 确定性分片（最后一片较短），没有重新压缩或修改源码；源包及每片 SHA/顺序/字节数均记录。
工具流式拼接到私有临时文件，校验每片和原整档摘要后才解包；缺片、乱序、篡改直接失败，不另下载替代。
GitHub 不逐文件展示压缩内容；可用仓内工具解压浏览或修改前核对来源。

- 官方地址、固定 SHA-256、字节数及成员数：[来源收据](source.json)。
- 许可入口：[COPYING](../../licenses/upstream/samba-4.23.0/COPYING)、
  [VFS 许可说明](../../licenses/upstream/samba-4.23.0/VFS-License-clarification.txt)。
  组件自身的其他许可/版权仍完整保存在源包内；这两个入口不替代逐组件条款。
- 来源验证：`python3 tools/samba-source/manage.py verify`。
- 解压浏览：`python3 tools/samba-source/manage.py extract --destination target/samba-source`。
  目标必须不存在；验证源包摘要、路径/链接边界后以临时目录原子发布，保留内部符号链接。
- 当前生产目标已改为 Debian 13 apt Samba 的匹配 SDK；本 4.23.0 归档保留作历史源码/核对材料，不是默认运行版本。详见 [SMB 说明](../../../docs/components/smb.md)。

上游发布包含 26 个重复的文档成员（各对字节相同），保留官方原包，不重新打包以改变来源摘要。
上游测试、文档和子工程只是原始材料，不是 ZXS 根 Cargo 工作区成员；解压及生成物仅进入 `target/`。
更新必须显式更新版本、官方 SHA、收据、许可副本并重新通过真实 ABI/FFI 门禁，不能运行时自动升级。
源码获取/解压不联网；构建仍要求预先安装文档列出的系统编译工具和开发库，不宣称全工具链离线。
