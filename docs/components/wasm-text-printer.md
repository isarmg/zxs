# WASM 普通无色打印测试组件

`wasm-text-printer` 源于 wasmprinter 0.245.1，只为绑定测试提供独立的 core 二进制 → WAT golden oracle。唯一公开操作入口 `print_bytes` 保持原默认平铺输出与双空格缩进；Legacy EH 的 Try/Catch/CatchAll/Delegate 普通打印保留，不借裁剪测试改变生产输入能力。

当前无可选特性：移除 component-model、validate/operand-stack、folded 实现；不保留 termcolor、Windows 控制台链或同名 shim。移除无人消费的 Config 呈现选项、offsets_and_lines、文件读取和公开 I/O/颜色适配入口。输出直接写入唯一目的地 `String`；移除单实现 Print/Adapter、空颜色钩子、输出位置与总返回 false 的自定义输出钩子，不另造框架。字符串追加不可失败；格式化写入使用说明该不变量的 expect。component 二进制明确拒绝。来源可达登记只服务验证，不假挂正式 web-bindings 运行依赖。

根 `test/foundation/wasm-text-printer` 保留上游 no_oom；上游 offsets/lines 测试随无消费者 API 一起移出当前套件，原源码留在发布档案。新回归覆盖固定普通输出、Legacy EH、无效 binary 和 component binary 拒绝。此前新写的 folded/operand-stack/自定义输出测试随未采用能力移除，不以自测给新 API 造需求。所有原 support WAT/golden 独立保留，本组件不生成或改写其预期。

## 具名错误边界

`Error` 不依赖 anyhow 或 `Box<dyn Error>`。`Parse(BinaryReaderError)` 保留原始解析错误作为 `source()`；非法 code section/page size/操作符终止、非 core encoding、未知 section/payload/dylink subsection、函数/locals/branch hints/type index 实现上限各有具体变体，不以字符串外壳代替类别。`String` 输出没有外部 I/O 或 fmt 失败边界，因此没有为假想写入失败新增 Error 变体或可注入 writer 接口。真实坏 binary 验证 source 身份，component、functions 和 locals 输入验证具体变体；原普通输出 golden 不变。
