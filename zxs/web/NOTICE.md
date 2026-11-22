# ZXS 管理页面视觉材料
本目录的样式源自 [原 ZXS 快照 8c385b1a](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/web/public/styles)，导航参考[原项目应用外壳](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/components/shell.rs)。实际复用的第一方 CSS 按 [Apache License 2.0](LICENSE-APACHE-2.0.txt) 分发。原 Rust 界面包另标有 Apache-2.0 AND MIT；这里只参照其界面布局，未复制或分发 Leptos 的源码。

来源：`src/web/public/styles/tokens/{tokens,tokens.dark,accessibility}.css`、`src/web/public/styles/components/{base,content-blocks,auth,system-status}.css`。字体来自用户系统。

## 管理页 G4 圆角

[`g4-corner.svg`](g4-corner.svg) 是页内定义，不是单独显示的图。其右上单位角取自原 ZXS [`profile.rs`](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/contours/profile.rs) 的弧长曲率构造：缓入、圆弧、反射缓出，圆弧比例 `p=0.4`，拼接参数 `t=3/8` 和 `5/8`。没有用贝塞尔拟合或通用 squircle，也没有引入原运行时。

从本项目目录用 `python3 web/make-g4-corner.py > web/g4-corner.svg` 重新生成。`t=0..1` 均分 64 段，包含两处拼接点，坐标最多保留 6 位小数。按原 [`geometry.rs`](https://github.com/isarmg/zxs/blob/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/src/interface/src/contours/geometry.rs) 的弦线插值误差界，归一坐标误差包含弦段 `S²K/(8·64²)`、数值积分和序列化，保守上界 `0.000134` 倍角宽（48 CSS 像素时小于 `0.007` 像素；不计浏览器栅格化）。这里是对原 G4 曲线的近似，SVG 折线本身没有 G4 连续性。

登录与概览各内联一份定义，给登录、手动提交入口和账号卡的四角及平面着色。每个目标保持自己的角宽；只有短边容不下时才按短边的一半等比缩小，不拉伸整张 SVG。装饰不裁剪正文或截获交互。共享配置列表使用一层普通圆角边框，不叠加不同曲线。无 CSS 容器尺寸支持或使用强制色时回退普通圆角。仅从本页引用，不增加资源请求或更改 CSP；生成器不属于构建或运行入口。尚未做浏览器验收。
