# ZXS 管理页面视觉材料
本目录修改并复用了 [原 ZXS 快照 8c385b1a](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/web/public/styles)的明暗基础色、无障碍焦点、登录卡、状态卡、表格和响应式布局；页头导航及单列主体参照[原项目应用外壳](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/components/shell.rs)。实际复用的第一方 CSS 按 [Apache License 2.0](LICENSE-APACHE-2.0.txt) 分发。原 Rust 界面包另标有 Apache-2.0 AND MIT；这里只参照其界面布局，未复制或分发 Leptos 的源码。

来源：`src/web/public/styles/tokens/{tokens,tokens.dark,accessibility}.css`、`src/web/public/styles/components/{base,content-blocks,auth,system-status}.css`。这里只保留改写后的展示，不含原字体、图标、旧后端、WASM、账号逻辑或 FILE 内容功能。字体来自用户系统，无外部请求。

## 管理页 G4 圆角

[`g4-corner.svg`](g4-corner.svg) 是页内定义，不是单独显示的图。其右上单位角取自原 ZXS [`profile.rs`](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/contours/profile.rs) 的弧长曲率构造：缓入、圆弧、反射缓出，圆弧比例 `p=0.4`，拼接参数 `t=3/8` 和 `5/8`。没有用贝塞尔拟合或通用 squircle，也没有引入原运行时。

从本项目目录用 `python3 web/make-g4-corner.py > web/g4-corner.svg` 重新生成。`t=0..1` 均分 64 段，包含两处拼接点，坐标最多保留 6 位小数。按原 [`geometry.rs`](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/contours/geometry.rs) 的弦线插值误差界，归一坐标误差包含弦段 `S²K/(8·64²)`、数值积分和序列化，保守上界 `0.000134` 倍角宽（48 CSS 像素时小于 `0.007` 像素；不计浏览器栅格化）。这里是对原 G4 曲线的近似，SVG 折线本身没有 G4 连续性。

Rust 每页只内联一份定义，每个目标用四块等宽高的角和中间平面着色。账号卡、登录卡及共享配置表外框保持各自的角宽；只有短边容不下时才按短边的一半等比缩小，不拉伸整张 SVG。装饰不会裁剪正文或截获交互；共享表保留 1 CSS 像素内缩的原生滚动区，内部普通圆角位于 G4 外框内。无 CSS 容器尺寸支持或使用强制色时回退普通圆角。仅从本页引用，不增加资源请求或更改 CSP；生成器不属于构建或运行入口。尚未做浏览器验收。
