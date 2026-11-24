# 退役的独立宿主观察命令

[仅静态] 已移除六个独立入口：`inspect-dirs`、`compare-files`、`storage-status`、`disks`、`hardware` 和 `disk-health`。它们不再显示在帮助中，也没有别名或后备行为。此更改不操作磁盘或移除旧安装。

离线维护仍核对原角色、挂载、锁和原件，这些是操作前提，不是对宿主容量、磁盘健康或文件内容的观察报告。池的整体验证是另一个功能，普通文件比较不能代替它。

- [移除前的命令和操作说明](https://github.com/isarmg/zxs/blob/f8faa958d2f7e9732a2e88757942905c6af907d4/helper/docs/guides/host-diagnostics.md)
- [原 NVMe 观察说明](https://github.com/isarmg/zxs/blob/f8faa958d2f7e9732a2e88757942905c6af907d4/helper/docs/guides/nvme-health.md)
- [原 Helper 源码、测试和许可证](https://github.com/isarmg/zxs/tree/f8faa958d2f7e9732a2e88757942905c6af907d4/helper)

历史链接不是受支持的命令或新的发行包。当前许可证与依赖归属说明仍见 [LICENSE](../../LICENSE) 和[依赖](../../DEPENDENCIES.md)。
