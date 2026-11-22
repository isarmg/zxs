# ZXS 浏览器界面

Rust/WASM 负责目录导航、层级路径和文件行。登录、共享设置、手动提交与回执沿用服务端表单；文件正文走 SMB。

[原界面快照 8c385b1a](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/web)的 `src/icons.rs` 提供三枚 SVG 路径，`src/files/view.rs` 和 `public/styles/components/file-manager.css` 提供目录导航与行样式设计。继续使用[旧版 G4 轮廓](../web/NOTICE.md#管理页-g4-圆角)。

## 服务接口

只有经鉴权的管理概览页引用固定模块。该页提供 `zxs-console-nav`、`zxs-account-link` 和目录正文；`zxs-console-secondary` 是版本及账号面板的共同容器。目录页将 `zxs-file-corner-source` 内的装饰移至列表，路径导航留在列表外。隐藏的 `zxs-browser-roots` 提供 `[data-zxs-share]` 的已启用共享及文本名，可带 `data-zxs-next`。`zxs-folder-load-error` 为简短的启动错误。

`GET /ui/v1/d/<ShareId>/<DirectoryId 或 ->/<页标识或 ->` 每次重新鉴权；页标识属于同会话同目录。成功响应为最多 128 KiB 的 `application/json`，含实际长度：`ancestors` 为根下到当前目录（不含共享根，最多 256 层），`entries` 为本页最多 32 个直接子项（`kind` 为 `directory` 或 `file`），`next` 为页标识或 `null`。每项仅 `id`、`name` 和所需类型。无会话返回 401。仅点击实际链接才打开新的手动提交表单。

## 浏览层边界与构建

目录状态、路径/分页标识校验和界面渲染留在 Rust。`console_host.js` 只接浏览器自身的 DOM、历史片段和有大小上限的目录响应：只发同源 GET，不跟随重定向；解码和渲染仅接收当前请求。它不代替服务端鉴权，也不生成或执行脚本文本。登录和手动版本表单不加载这个模块。

独立浏览器 crate 没有第三方 Cargo 包。使用受核对的本地 Rust 和官方 `wasm32-unknown-unknown` 标准库离线构建。构建把同源浏览器接口与 Rust WebAssembly 模块写到原四个资产位置；[正式构建](../BUILD.md)核对来源和摘要后嵌入同一 ZXS ELF。
