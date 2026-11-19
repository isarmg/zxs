# ZXS 管理页面视觉材料
本目录修改并复用了 [原 ZXS 快照 8c385b1a](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/web/public/styles)的明暗基础色、无障碍焦点、登录卡、状态卡、表格和响应式布局；页头导航及单列主体参照[原项目应用外壳](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/components/shell.rs)。实际复用的第一方 CSS 按 [Apache License 2.0](LICENSE-APACHE-2.0.txt) 分发。原 Rust 界面包另标有 Apache-2.0 AND MIT；这里只参照其界面布局，未复制或分发 Leptos 的源码。

来源：`src/web/public/styles/tokens/{tokens,tokens.dark,accessibility}.css`、`src/web/public/styles/components/{base,content-blocks,auth,system-status}.css`。这里只保留改写后的展示，不含原字体、图标、旧后端、WASM、账号逻辑或 FILE 内容功能。字体来自用户系统，无外部请求。
