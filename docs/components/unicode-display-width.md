# Unicode 诊断显示宽度

`unicode-display-width` 源于 unicode-width 0.2.2，只保留当前 WAT 诊断需要的正常 Unicode 字符/字符串显示宽度。当前消费者 `wasm-text-parser::error` 调用 `UnicodeWidthStr::width` → `str_width` → `width_in_str`/`lookup_width`；不调用原 `width_cjk`，不能用字节数或字符计数替代。

源码始终为 `#![no_std]`，当前无 feature。删除无消费者的 `width_cjk` 公开能力及其 Ambiguous=2 专用算法、CJK root、4个附加 middle 和23个附加 leaf 表；保留正常16个 middle、163个 leaf。普通中文宽字符、组合符、emoji/ZWJ 的语义不是该可选 CJK 模式，不裁掉。`lookup_width`、`single_char_width`、`width_in_str`、`str_width` 四个正常算法函数与发布原件逐字相同。原生成表、脚本、许可和 CJK 实现完整保存在中央原始档案，不执行生成脚本重造表。

根 test 保留表规范化测试、34个原测试的正常 width 断言和 emoji 数据；CJK 专用断言移出当前套件，不宣称原双模式全套通过。数据查找固定到根 test。保留一个普通宽度 Rustdoc 示例；专门展示 width_cjk 的示例随能力归档。原 nightly benchmark 与生成器不作为当前执行入口，rustc 内部依赖入口不纳入当前源码。许可保持 MIT OR Apache-2.0，原版权及 Unicode 说明不改写。
