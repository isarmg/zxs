# 辅助工具内置许可与版权材料

本片从 `92fc12ec` 独立推进 H08/T10，不改已交付但未发布的同载荷程序修复候选。它使正式 helper 除准确 ZXS 程序外，还携带可验证的中央许可/通知归档与文件清单。**未审查、编译、格式化、运行测试或 CI，未生成真实归档或发行载荷，未操作实际主机。** 新增 12 项测试源码并调整已有 fixture/构建回执用例，均未执行。

## 固定材料范围和未关闭事项

归档仅包括以下准确 Git tree 中的普通 blob：

- `compliance/README.md`：含目前尚未关闭的许可/版权资料复核说明；
- `compliance/NOTICE.md`：现有通知全文，字节不改；
- `compliance/provenance/origins.json`：中央来源身份与许可映射；
- `compliance/licenses/` 下全部已跟踪的普通文件，包括第一方、原始上游、共享、补充及字体许可材料。

不读取 untracked 文件，不把工作区同名修改当作已提交来源，也不跟随提交树中的链接或子模块。路径不可绝对、穿越、含反斜杠/控制字符或越出上述固定范围。项目 Apache-2.0、中央通知/来源和至少一份 upstream 许可证必须齐全，否则构建拒绝。

**这不是法律完整性、合规性或独立许可审查结论。** 清单固定 `legal_completeness_attested=false`，指出 `compliance/README.md` 是未关闭事项来源，保留其中关于 `erased` 等资料的现有说明。不改写版权/许可证、不删除来源材料，也不把 archive 当作满足 GPL 对应源码义务的替代物。

它不是完整源代码压缩包、VFS/SDK、OS 依赖、完整部署材料或系统恢复包。当前 Release 中的完整来源/材料资产不得因这份内置许可切片被直接删除。

## 非递归构建与准确绑定

`release-native.py` 继续先构建 ZXS。随后 `helper-legal-materials.py` 从已固定 commit/tree 的 Git blobs 准备许可快照，清单绑定版本、目标、source commit/tree、准确 ZXS SHA-256，以及各文件的路径、Git blob ID、长度和 SHA-256。

归档为 gzip + tar，固定条目顺序、uid/gid/mtime=0、mode=0644、空 uname/gname；PAX 只承载必要路径表示。`MANIFEST.json` 的字节与独立清单一致。不执行被归档内容，也不递归包含 helper 自身。相同输入与同一生成实现有确定输出，不宣称跨 gzip/工具链版本的完全可复现构建。

构建脚本要求现有 payload 五项输入加上四项材料输入：

```text
ZXS_HELPER_MATERIALS
ZXS_HELPER_MATERIALS_SHA256
ZXS_HELPER_MATERIALS_MANIFEST
ZXS_HELPER_MATERIALS_MANIFEST_SHA256
```

两份路径均为绝对路径、非链接普通文件；build.rs 有界读取、核对 SHA-256、复制进 `OUT_DIR` 后将快照字节编入 helper。仅提供程序而遗漏材料的正式嵌入构建拒绝；没有 payload 的开发构建也不接受零散材料设置，状态明确为不存在，不读取旁边文件或运行时联网补齐。

此绑定依赖受控 native 构建入口及其确定输入；不是独立签名或对任意手工伪造构建环境的来源认证。运行时重新核验程序和两份材料的摘要，以及版本/commit/tree/target/payload 绑定，输出中保留该保证范围。

helper 构建后，native 入口再核程序、归档和清单都未改变，才写成功 `BUILD.txt`。增加 `native-helper-license-materials-embedded`、归档/清单 SHA-256、文件数及 source commit/tree；资产验证器要求这些字段完整、与已有 native/Web 来源一致。失败不留下旧成功构建回执。没有修改 CI 触发器、执行 CI、创建 Release 或发布资产。

## CLI 与独立 Web 交付

```text
zxs-helper install payload
zxs-helper install materials --output /用户指定的已存在父目录/zxs-license-materials.tar.gz
zxs-helper install materials-manifest
```

`materials` 先核验全部内置字节，然后复用 CLI 的原子 no-clobber 新文件发布，输出文件 0600。已有文件、链接等不被覆盖；输出失败保留原层的发布/持久化错误语义。`materials-manifest` 向 stdout 输出准确原始 JSON 清单，不重新序列化或宣称写入文件已持久化。

Web 程序安装卡片显示归档来源/长度/SHA-256，提供下载归档与清单按钮。`POST /api/install/materials-download` 和 `POST /api/install/materials-manifest` 都只接受空 JSON 对象，沿用同源/CSRF/IP Host、单阻塞工作 owner 与退出排空。响应给出准确摘要头和 attachment 文件名；浏览器核对响应摘要头与长度再保存。

服务器不接受输出路径、上传材料或解包请求，不写任何下载文件，不创建安装目录，不触碰数据/配置/账户/服务。所有材料视为可下载的公共构建内容，没有执行或 HTML 渲染入口。HTTP 控制台仍无访问认证/TLS，摘要头核对不等于经过敌对网络的端到端来源认证。

## 预算、资产边界与下一步

最多 8,192 个文件，单文件 16 MiB、原文总量/压缩归档各 32 MiB、清单 4 MiB、路径 512 UTF-8 字节。生成器先核 Git blob 大小，再读正文，不递归扫描运行池。构建阶段暂存输入/归档及清单；下载核验为 O(内置程序 + 归档 + 清单字节数)，在已有单阻塞任务配额内执行，HTTP body 借用不可变内置字节，不在服务器复制解包目录。

归档/清单只在忽略的 native 构建输出中作为中间材料，随后嵌入 helper；**没有新增第五类顶层 Release 资产**，也未声称既有资产名单已收敛为最终四类。最终四类资产重构、VFS/依赖安装包、真实构建与分发验证仍待完成。发布暂停期间只交源码候选，不通过别的通道上传、推送或绕过批准。
