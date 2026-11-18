# ZXS 自动留版本选择指南

**默认选手动提交。** 只有当软件直接把原文件保存到 ZXS 的 SMB 共享、保存不频繁，而且这些保存点值得回看时，才考虑自动留版本。不要只看扩展名。

软件的“自动保存”解决忘记按 Ctrl+S 的问题；ZXS 的“自动留版本”决定哪些**已保存的工作稿**进入文件历史。选手动提交不会关掉软件的自动保存或崩溃恢复，也不妨碍继续保存工作稿。

## 按实际用法选择

| 文件和软件 | 实际保存方式 | ZXS 建议 |
| --- | --- | --- |
| Word、Excel、PowerPoint：`.docx`、`.xlsx`、`.pptx` | Microsoft 365 的 AutoSave 主要用于 OneDrive 或 SharePoint Online；直接打开普通 SMB 文件服务器上的文档通常不启用这一功能。[Microsoft 365：什么是 AutoSave？](https://support.microsoft.com/en-us/office/collab-files/what-is-autosave) | **可选自动**：如果平时只在完成一段工作后保存，而且想回看这些节点。 |
| VS Code：`.py`、`.rs`、`.md`、`.txt` 等脚本和配置 | “延迟后自动保存”可能在编辑停顿时反复写原文件。停顿不一定是值得留档的时刻。[VS Code：Save / Auto Save](https://code.visualstudio.com/docs/editing/codebasics#_save-auto-save) | **建议手动**：完成一次有意义的修改再提交。 |
| AutoCAD 图纸和 Blender 项目：`.dwg`、`.blend` | AutoCAD 自动保存通常另写 `.sv$`；Blender 会生成临时恢复副本。它们不会自动给原图留一版；落在共享上时还可能产生额外写入。[AutoCAD 自动保存与恢复](https://www.autodesk.com/support/technical/article/caas/sfdcarticles/sfdcarticles/Understanding-AutoCAD-backup-and-autosave-files.html) · [Blender 数据恢复](https://docs.blender.org/manual/en/latest/troubleshooting/recover.html) | **建议手动**：按设计阶段提交原文件，保留软件自身的恢复功能。 |
| Photoshop 和大型计算工程：`.psd`、`.psb`、仿真文件 | Photoshop 的定时恢复和后台保存是两项功能。大项目频繁留版本成本高；不确定软件怎么写时，先试副本。[Photoshop 文件保存首选项](https://helpx.adobe.com/photoshop/desktop/save-and-export/save-files/file-saving-properties-and-preferences.html) | **建议手动**：里程碑提交。 |
| 运行中的虚拟磁盘：`.vhd`、`.vhdx` | 来宾系统会持续写镜像；对单个文件留版本不能代替虚拟机的一致性备份。[Microsoft：SMB 与 Hyper-V 场景](https://learn.microsoft.com/en-us/windows-server/storage/file-server/file-server-smb-overview) | **不对运行镜像开自动**；把已完成的一致性备份或导出产物放入 ZXS。 |
| 正在使用的数据库：`.sqlite`、`.db` | 例如 SQLite 的已提交事务可能还在 `-wal` 中；只留主文件不一定能还原当时的数据。[SQLite：Write-Ahead Logging](https://www.sqlite.org/wal.html) | **不对运行数据库开自动**；使用数据库自身的备份或一致性导出，再保存产物。 |
| 日志与缓存：`.log`、`.tmp`、缓存目录 | 文件持续追加或频繁覆盖，多数中间状态不值得回看。 | **不建议自动**；要归档时，手动保存已轮转的日志或最终导出结果。 |

## 用一个副本确认

同样是 `.md`：每周才更新一次的共享作业说明，可以考虑自动；编辑器每隔几秒就写一次的开发笔记，更适合手动。对不熟悉的软件，可以这样判断：

1. **准备副本。** 在 ZXS 的 SMB 测试目录放一份不重要的文件。用平时实际使用的软件和设置，不用正式项目文件。
2. **分两轮看原文件。** 第一轮只修改，不按保存，至少等过一个软件自动保存周期；第二轮主动保存并关闭。每轮用另一个窗口或客户端重新打开，核对原文件的**内容**，也看同目录是否多了恢复文件。只看修改时间不够。
3. **按观察结果选。** 原文件频繁变化，选手动；如果原文件只在少数值得回看的时刻变化，可以试自动。如果只有恢复文件变化，不必为原文件开自动。虚拟机和数据库仍优先使用专用备份。
4. **回看效果。** 做几次典型编辑，看 ZXS 历史留下多少有用版本，并打开一个版本确认能读。如果大多是中间稿，就切回手动。不要为了减少 ZXS 历史而关闭软件的崩溃恢复。

## 保存、历史和备份不是一回事

- **保存成功不代表一个历史节点已经生成。** 自动模式可能只处理最新的已保存稿，不保证每次 Ctrl+S 都留一版。本机恢复副本或普通 SMB 写包，也不等于 ZXS 工作稿已保存。慢盘上请等软件完成保存，再退出或关机。
- **正式版本要考虑整份正文的成本。** 读取工作稿不等于复制整个文件；但当前正式版本仍保存独立完整正文。例如，10 GB 文件留 3 个新版本，单份正文可能增加约 30 GB，尚不计暂存、元数据和其他副本。[ZXS 完整文件存储规则（固定提交）](https://github.com/isarmg/zxs/blob/321304d57f9df32ae4446ba246108095d690f492/docs/08-full-storage-and-disk-format.md)
- **先在非生产副本上验证。** 工作稿的大区段改造仍在开发，本指南不是大文件性能、生产发布、可恢复备份或固定间隔留档的承诺。正式数据仍需独立、实际验证过的备份。
