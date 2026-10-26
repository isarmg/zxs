# ZXS Web 绑定生成器

从 wasm-bindgen-cli 0.2.129 的正式绑定命令提取，只接受 ZXS 的 `--target web --no-typescript --out-name NAME --out-dir DIR INPUT` 契约，继续使用同版 `wasm-bindgen-cli-support` 解析和生成绑定。原始发布包及逐文件映射见 [PROVENANCE.json](../../compliance/provenance/packages/zxs-bindgen-0.2.129/PROVENANCE.json)，原始 MIT / Apache 许可随源码保留。

删除 test-runner、wasm2es6js、HTTP 测试服务器、网络下载及其依赖；该正式构建工具没有 rouille、multipart、ureq 或 TLS 依赖。输入验证、WASM schema 检查及生成错误仍由原有支持库处理。Web 绑定参数与原 CLI 对应调用保持一致，包括默认 Wasm URL 和 reference-types 行为。

```sh
cargo test --locked --offline -p web-bindings
bash .github/scripts/rust-build-tool.sh wasm-bindgen
```

这是 ZXS 自维护的编译辅助代码；版本字符串保持 `wasm-bindgen 0.2.129`，以匹配 Web crate 的 schema 版本。它不是上游三个通用命令的完整替代品。

`--out-name NAME` 只接受非空 ASCII 字母、数字、下划线和连字符；默认从输入名推导时也经过同一校验。不接受目录分隔符、点段、引号或脚本字符，避免输出越界和生成 JavaScript 中的名称拼接注入。

<a id="modern-web-runtime-contract"></a>

## 现代 Web 运行契约

当前支持库和正式 CLI 都只生成 Web ES module，不保留 Node／bundler／Deno 输出模式，也不新增 modern/legacy 开关。生成的类与闭包直接构造 `FinalizationRegistry`，不再在缺 API 时安装 no-op register/unregister；缺失会使绑定模块导入失败，交由资源引导器显示启动失败，不能以泄漏闭包的方式继续运行。GC 最终清理不承诺具体时刻，显式 owner 清理、RAF 取消和弱引用仍保留。

普通 WASM 内存直接使用 TextEncoder.encodeInto，不生成缺方法时的 encode-copy polyfill；未被消费者使用的 EncodeInto Test/Always/Never 策略及配置 API 撤销。共享 WASM 内存的 encode 后复制路径单独保留，它处理 host 编码目标的数据限制，不是旧浏览器兼容。应用启动边界要求 encodeInto，生成胶水本身的调用缺失也直接失败。

网络 `Response` 输入直接使用 `WebAssembly.instantiateStreaming`。缺此 API、错误 MIME、网络／编译／链接错误均明确失败，不改走字节重试；HTTP 非成功状态仍包含状态码、状态文本和 URL，streaming 异常原样传播。调用者显式传入 bytes 或 `WebAssembly.Module` 的现有输入契约仍用 instantiate，它不是网络失败后的兼容路径。服务必须正确发送 `application/wasm`。

`.github/scripts/check-web-bindgen-scope.py` 编译真实 WASM 夹具并用正式生成器检查类／闭包输出，随后执行 `test/tools/web-bindings/runtime/modern-loader.mjs`：Node 仅用于无 socket 的生成胶水语义回归，覆盖能力缺失、MIME／HTTP／WASM 错误传播及正常 Response／bytes／Module，不是 Node 产品输出，也不替代真实 Chrome。当前没有需批量重写的 JS golden；保留的 WAT 预期涉及转换算法，不因删除 JS 兼容路径而改写。生产源码修改通过来源工具刷新当前收据，原始来源和历史证明不变。
