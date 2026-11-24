# 退役的卷镜像工具与本机网页

Helper 不再提供 FAT32、exFAT、ext4、XFS、NTFS、Btrfs 镜像工具、检查工具清单和本机网页，也不再交付它们的私有发行包或依赖。当前首设和维护是本机终端动作，不会启动下载器或改用宿主的检查器。此更改不删除现场数据、镜像、已有安装或原源码记录。

移除前的固定 Git 快照是 [`011c58f`](https://github.com/isarmg/zxs/commit/011c58f2e0f5df80e939fcae058e94d045b5ff68)。可从这一版本独立追溯：

- [镜像及工作副本源码](https://github.com/isarmg/zxs/tree/011c58f2e0f5df80e939fcae058e94d045b5ff68/helper/src/volume_images)、[本机网页和用途边界](https://github.com/isarmg/zxs/blob/011c58f2e0f5df80e939fcae058e94d045b5ff68/helper/docs/disk-web.md)。
- [Debian 13 amd64 打包配方和版权/对应源码要求](https://github.com/isarmg/zxs/blob/011c58f2e0f5df80e939fcae058e94d045b5ff68/helper/packaging/debian13-amd64/README.md)；[上游包和摘要清单](https://github.com/isarmg/zxs/blob/011c58f2e0f5df80e939fcae058e94d045b5ff68/helper/packaging/debian13-amd64/lock.json)。
- [原 Cargo 依赖、精确版本和许可](https://github.com/isarmg/zxs/blob/011c58f2e0f5df80e939fcae058e94d045b5ff68/helper/DEPENDENCIES.md)，包括现已移除的 `httparse`；[原 Helper 项目许可](https://github.com/isarmg/zxs/blob/011c58f2e0f5df80e939fcae058e94d045b5ff68/helper/LICENSE)。

历史记录不是新的发行包或再次授权。若独立分发旧工具，仍需按实际分发组件保留适用版权、许可、例外、通知和对应源码；不能用此索引替代原文。
