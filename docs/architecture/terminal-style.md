<a id="terminal-style-capabilities"></a>

# 终端样式能力

`terminal-style` 统一维护 ANSI 样式、终端解析、原生输出适配与进程级颜色策略。原 anstyle、anstyle-parse、anstyle-query、colorchoice 和 anstream 的包边界已移除，不保留转发包。

- 默认：无依赖的 `no_std` 样式类型与单一原子 `ColorChoice` 策略。
- `std`：基于 `std::io` 的样式写入器。
- `parser`：使用 `alloc` 和 ASCII 字符累积的 `no_std` ANSI 解析器。
- `parser-core`：通过 arrayvec 提供固定容量的 OSC 存储，不需要 `alloc`。
- `parser-utf8`：通过 utf8parse 累积 UTF-8 字符。
- `output`：显式的标准库输出能力，带 UTF-8 解析，由命令行颜色与日志颜色特性使用。原生运行验证目标为 Linux amd64；浏览器 WASM 可以编译，但原生流检测不是浏览器控制台桥接。

解析状态转换表与终端环境查询属于私有实现。消费者使用 `terminal_style::output::{AutoStream, stream::IsTerminal}` 及根 `terminal_style::ColorChoice`。命令行样式和日志共用样式类型与颜色策略状态。标准 ANSI 转义及 NO_COLOR、CLICOLOR、CLICOLOR_FORCE、TERM、CI 环境变量语义保持不变。

输出短写后的解析状态对应实际消费的输入前缀；调用者重试剩余字节时不丢正文。字符串分块尚未产生可打印内容时保留未完成控制序列的状态。参数迭代器的长度提示按剩余参数组给出合法上下界，不把子参数槽数当作参数组下界。

不支持的 Windows 控制台适配、查询实现及未使用的流格式宏已移除。旧源码的精确字节、哈希与路径保留在来源材料和经认证的整合收据中。原始上游说明、许可证、版本、来源哈希与 manifest 不改写。13 份原始外置测试均保留归属，并与生产源码分离。不可用的上游 proptest / 代码生成工具仅作为测试来源保留，不计作已执行测试；维护的确定性协议与字节边界测试无需这些依赖即可执行。

运行 `.github/scripts/check-terminal-style.py` 检查七种原生特性组合、四种 WASM 组合、无依赖默认解析、真实命令行/日志消费者，以及不允许的 API / 特性。foundation-consumers 测试另行验证真实输出去色、直通和终端检测。
