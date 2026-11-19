# 原账户与口令验证

账户只认[便携原登记上明确确认的决定](portable-decisions-v1.md)。首设须在原登记的生命周期 EX 内取得一次独立授权，明确选择管理范围。维护关联号、存储 UID 或数据库里只有一个人，不授后台/FILE 或恢复权限。另一名普通用户必须有自己的原 AccountId、准确登录名和凭据决定。没有可用的网络登录或账号发行端；旧 MAINT-1 的空池观察不再解释账户帧，没有兼容读或升级代码。

登录名目前限 1..64 字节，小写 ASCII 字母、数字、点、下划线或连字符，首字符为字母或数字。口令按输入的 UTF-8 原字节限 1..1024 B，不截断、规范化或写入帧/回执。真正的首次设密/重置须新建独立随机 16 B 盐；同一原单中断重试不得换盐或改变当初授权。盐和 32 B 验证标签仅在受保护的凭据决定中；可查询的请求摘要既不含这两项，也不含明文或快速猜测材料。未知/截断/未确认尾不给账户或数据权限，不能跳过它仅读首管。

本片的口令验证依赖目标 Linux 的 [libargon2 参考库](https://github.com/P-H-C/phc-winner-argon2/blob/20190702/include/argon2.h)，以 20190702 的 argon2id_verify C ABI 为依据。这是当前代码真实的目标构建/认证依赖，不是已内置的资格方案。常规动态链接时，构建机/交叉 sysroot 须有目标架构的 [libargon2.so 链接入口](https://packages.debian.org/trixie/amd64/libargon2-dev/filelist)；离线交付物须随附已核准的同架构 ABI 库及适用许可，或明确要求宿主预装等效 [libargon2.so.1 运行包](https://packages.debian.org/trixie/amd64/libargon2-1/filelist)（链接示例为 Debian trixie amd64）。不能承诺无需运行库的单文件；没有 ABI 不用自写或弱散列后备，安装者须在启用前核对而不是让 ZXS 自动下载安装。本项目不复制、修改或内置该库的源码；上游[许可证](https://github.com/P-H-C/phc-winner-argon2/blob/20190702/LICENSE)为 CC0-1.0 或 Apache-2.0。使用固定参数重建 PHC 并调用库的 argon2id_verify，不从输入接收 PHC、参数或任意摘要；不依赖命令行也不自行下载工具。一次进程内最多一个 64 MiB 的验证，繁忙会拒绝，不能代替网络登录所需的限流。合成[便携首设向量](../tests/vectors/portable-first-decision-v1.txt)的盐与验证标签取自 [argon2-cffi 25.1.0 公开示例](https://argon2-cffi.readthedocs.io/en/stable/howto.html)；相应公开测试输入仅供夹具使用，绝非默认账户口令，不进入原决定、请求或回执。这里没有构建、实盘或独立写端互通结论。
