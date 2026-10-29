# HTTP 上游回移：证明与运行验证

[源码治理入口](../tools/rust-sources.md#http-owner-source-maintenance-chain) · [外置测试归属](../EXTERNAL_TEST_SOURCES.md)

本页给出回移的维护／验收步骤，不是通过报告。真实结果须绑定执行提交、工具链、命令和日志。
回移对象是 hyper #4222（`97fff93e53f9e6a993f2d6a25d50883b7b8752a7`）；
当前包和扫描身份仍为 `http-transport@1.12.0` / `hyper@1.12.0`。
固定 schema 2 收据：

```text
c75e73b0b8aab76fc3934f6c72620127795b8c95ec7bc0f52d173664e477938d
```

该收据的 `consumer_tests.executed` 是空对象。后来执行的门禁结果独立存档，
不修改此收据、前序收据、origins、NOTICE、许可证、Cargo 清单或锁文件。

## 来源生成与输出边界

在有固定工具链的可信 GitHub 验证任务中，从完整候选源码 checkout 执行，
不在归档或本地受限执行器运行脚本，也不手写摘要来代替生成。
先确认回移源码、抽取测试和映射已提交，收据 ID 已按审阅结果追加到
`proof_legacy_maintenance.RECEIPTS`；不重复 apply 同一修复。

```sh
python3 tools/rust-sources/manage.py refresh
python3 tools/rust-sources/foundation_coverage.py --write
python3 tools/rust-sources/architecture.py refresh
python3 tools/rust-sources/documentation.py refresh
```

上述四条生成命令只允许产生以下输出变更；不要求本次每个文件都改变：

```text
compliance/rust-sources.json
docs/THIRD_PARTY_RUST_SOURCES.md
test/ci/fixtures/foundation-test-coverage.json
docs/validation/foundation-test-coverage-map.md
docs/architecture/foundation-consumers.md
docs/dependencies/code-generation.md
docs/dependencies/collections.md
docs/dependencies/concurrency.md
docs/dependencies/core-types.md
docs/dependencies/cryptography.md
docs/dependencies/data-storage.md
docs/dependencies/diagnostics.md
docs/dependencies/integrated.md
docs/dependencies/networking.md
docs/dependencies/platform.md
docs/dependencies/text-unicode.md
docs/dependencies/tooling.md
docs/dependencies/web-runtime.md
```

审阅生成差异：HTTP 维护链追加固定收据；当前三处文件摘要、外测映射摘要、
抽取测试字节核算与收据 after 一致。测试覆盖区分项目新作的公开 API 回归与保留但
未由本门禁执行的 `dispatch/tests_770.rs`；不将清单中的「有执行入口」视为本次运行通过。
包身份、来源、许可、历史摘要、产品依赖和其它所有者不应因本次回移改变。

## 证明失效负测与正常验证

```sh
python3 -m unittest discover -s test/tools/rust-sources -p 'test_source_backport.py' -v
python3 -m unittest discover -s test/tools/rust-sources -p 'test_legacy_http_maintenance*.py' -v
bash .github/scripts/check.sh rust-sources
python3 tools/rust-sources/source_backport.py verify \
  --receipt c75e73b0b8aab76fc3934f6c72620127795b8c95ec7bc0f52d173664e477938d
```

负测包含：重定向／穿越／绝对路径且重算整个 replay；篡改及省略区域并重新生成
冲突摘要、解决项和结果；失效的映射证据；未路由目标；叶节点／祖先目录／映射链接；
文件消失；既有收据不可覆盖；失败证据保存失败不替换原异常；正常适配器拒绝
重算后的伪造切片，并且不递归 `layout.read`。拒绝前后核对原文件内容、收据及失败记录。
既有链测试还覆盖父链／完整快照不连续、修改未知外测、修改 correction 覆盖的外测、
新增生产文件、修改 manifest、各输入漂移及 refresh 的紧邻前序窗口。

生成物提交后，针对该精确 HEAD 再执行干净重放：

```sh
python3 tools/rust-sources/source_backport.py verify \
  --receipt c75e73b0b8aab76fc3934f6c72620127795b8c95ec7bc0f52d173664e477938d \
  --clean-copy
```

此命令从 `git archive HEAD` 创建临时目录，运行其中的回移验证和完整来源验证，
不会使用未提交的工作树补丁。若提供 `--upstream-clone /path/to/verified-local-clone`，
还会核对记录的上游 base/fix 字节；没有 clone 时仅证明已收据化材料的认证重放，
不能声称进行了新一次上游远程核验。报告可用 `source_backport.py report --receipt …
--output /outside/repository/empty-directory` 保存到仓库外。

## 实际行为与消费者

```sh
rustfmt +1.99.0 --edition 2024 --config skip_children=true --check test/architecture/http-transport/backport.rs
python3 .github/scripts/check-http-transport.py
cargo check --locked --offline -p storage-engine -p storage-server -p storage-rescue
cargo test --locked --offline -p storage-client -p storage-server
```

HTTP 门禁继续执行原 25 项行为测试、2 项 FFI 回归和 18 项 primitive 测试及重复竞态，
新增 5 项项目公开 API 测试使行为总数为 30。新测试不用新依赖、网络或固定 sleep：
reset / EOF × 普通驱动 / `poll_without_shutdown` 四种组合在协作预算耗尽时强制
drain Pending，核对唤醒、未发送请求取消、原始 reset 错误来源和 shutdown 次数；
第五项在两种驱动下保持 upgrade 的未关闭／pending readiness 语义与缓冲字节。
完整 foundation-async 和 workspace 验证仍分别通过既有 CI 入口执行；这些结果
不能代替安全公告扫描、真实部署或实际网络／SDK 验收。

来源维护仅在真实输入边界重新认证；可信内部 replay 复用已验证字节。
区域认证在原有有界 owner 读取／逆向过程后进行，不新增文件系统扫描或同步调用。
它按登记的 upstream 文件和抽取区域查找历史切片，保留既有字节／文件／补丁预算。
