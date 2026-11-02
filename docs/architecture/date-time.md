<a id="datetime-capability"></a>

# 日期与时间能力

`date-time` 负责日历运算、时间戳、UTC 与显式固定偏移、格式描述、解析、格式化及可选序列化。Rust 导入名为 `date_time`，与 `std::time` / `core::time` 及 tracing 私有 `fmt::time` 不同。

日历单位、有界整数、宽度感知显示与整数转换共用编译/特性边界；除公开的日历单位与工具函数外，辅助模块私有。数值辅助层只提供日期运算所需的扩宽与低位截断。

<a id="supported-consumers-and-features"></a>

## 消费者与支持特性

- storage-engine 解析和格式化持久化容器时间戳。
- storage-server 格式化线上时间戳及遥测时间。
- tracing 的 `time` 特性为真实 `UtcTime`、`OffsetTime` 采集器启用 date-time；固定偏移由调用者显式提供。
- 序列化门禁使用真实 data-serialization/json-codec，包含 date-time JSON 往返。
- 默认 `std` 启用 `alloc`。禁用默认特性的日历/时长运算不需要分配器或标准库；解析也不需要二者，动态分配的格式描述需要 `alloc`。
- `formatting` 需要 `std`；`large-dates` 在包内扩展年份范围，无跨包特性转发。
- `serialization` 直接使用 data-serialization，包括私有有界表示。可读及知名格式适配器另启用格式化/解析；紧凑序列化不需要 `std`。
- `wasm-bindgen` 保留真实 js-sys Date 互操作和浏览器时钟路径。原生目标为 Linux amd64；浏览器 WASM 服务于 Web 客户端，不是另一种原生平台。

固定九位小数秒在数值转换前拒绝全部非 ASCII 数字字节；非法输入返回解析错误。`Period` 的大小写严格性独立于输出大小写：严格小写仅接受 `am`/`pm`，严格大写仅接受 `AM`/`PM`，忽略大小写模式接受两者的大小写组合。

<a id="explicit-unsupported-boundary"></a>

## 明确不支持的能力

不提供操作系统本地时区发现、`local-offset` 或时区刷新 API。请求系统偏移时不能静默以 UTC 代替。调用者使用 UTC 或显式固定偏移；命名时区、夏令时及变化中的本地时区解析需要单独设计。

不提供随机、属性测试及日期过程宏特性；应使用构造器和运行时格式描述解析。负向门禁验证不支持的特性、本地时间方法及私有实现模块无法被消费。

<a id="evidence-and-limits"></a>

## 验证与限制

`.github/scripts/check-date-time-family.py` 检查原生/WASM 隔离特性闭包、日历和时间戳边界、真实序列化，以及 tracing 的 UTC / +05:30 格式化。WASM 检查是编译证据，不是浏览器执行。tracing 与序列化门禁保留各自消费者，现解析到 date-time 包。

项目能力测试位于 `test/date-time`。保留的基准未启用；完整上游全特性单元目标仍依赖不可用的 `serde_test2` 和旧 JSON 测试框架，不能视为已通过。独立门禁使用真实序列化、格式化与解析实现。

`format_into` 返回实际写入的字节数，包括小数秒；回归比较 UTC 和 +05:30 中 1、3、9 位小数秒以及纪元前时刻。来源与许可证见集中合规材料。
