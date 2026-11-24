# 本机代码包：ZIP32 Deflate

`zxs-deflate-zip32/v1` 是普通目录的打包配方。结果是标准 ZIP，不是旧 `.zxs` 仓库；当前只在本机生成，不解包、不上传，也不设置特殊服务端文件类型。

## 目录和忽略

- 从明确选择的普通目录扫描。根 `.zxsignore` 是至多 64 KiB 的 UTF-8，按 Gitignore 风格处理；不读取环境、全局 Git 配置或嵌套规则。`.zxsignore` 本身可作为普通内容，不是私有清单。
- 默认忽略 `node_modules/`、`target/`、`.venv/`、`__pycache__/`、`.DS_Store`、`Thumbs.db`、`*.swp`、`*~`。忽略的目录不遍历。
- 硬跳过 `.git`、`.zxs-remote-state`、`.workspace.lock`、`workspace.sqlite` 及其 `-journal`、`-wal`、`-shm` 名。拒绝非忽略的链接、特殊对象和路径别名。
- 条目必须是精确 UTF-8 相对路径，不改名或归一化。段最多 255 UTF-8 字节，路径最多 1,024 字节。不收控制字符、若干不便携字符或结尾点空格。
- 已有限量：总计 384 MiB 普通正文、32,000 次条目访问、深度上限检查 48。大小写别名检查使用 Unicode 小写映射，不代表 Windows/macOS 名称或提取资格。

## 字节格式

| 项 | 规则 |
| --- | --- |
| 顺序与目录 | 按相对路径的 UTF-8 字节升序；保留空目录，目录名加 `/`。 |
| 压缩 | 常规文件用原始 Deflate（ZIP 方法 8），级别 6，空文件也使用 Deflate；目录用 Store（方法 0）。 |
| 元数据 | 写 UTF-8 标记、1980-01-01 00:00:00，文件权限 0644、目录 0755。源权限和 mtime 不进入 ZIP。 |
| 边界与校验 | 普通 ZIP32 头、CRC32、中心目录和结束记录；没有自定义字段、注释或内嵌清单。写完并同步整包才从头读回计算长度和 SHA-256。 |

本机差异清单按普通路径记录成员；新增、变化、移除与上一次成功打包该原路径的清单相比。它不是原 ZIP 的成员或服务端历史。确认后比对原目录变化；要生成新名字，不能覆盖晚到的已有文件。

源码 [bundle.rs](../../src/bundle.rs) · [独立 ZIP 读回](../guides/linux-ext4-qualification.md) · [PKWARE ZIP APPNOTE](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT)。
