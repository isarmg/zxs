# 一期 HTTP 文件保存与节点标签回归

## 准确来源与范围

生产源码固定为 `2d983a9f19dc7c2ff2a4a623ada05feac37c6176`，tree
`df9e69a8b2d3bc4166054bca4c4b375a5771d117`。正常保存/标签测试来自
`b5070074a9069ff595756870f0739efb069601ff`；两项真实 Engine 拒绝回归来自
`71ff2583c04a415c3954ab69568ad9ae9a8bfa44`，其测试断言修正为
`6dea4d50dcee14fb88314905f590f9c406bebe62`。本次实际编译、运行的测试源码是
`6dea4d5`，Engine、Contract、Platform、SMB 和 Server 生产源码与统一基线相同。

独立目标 `phase1_save_tags` 位于
`test/server/integration/phase1_save_tags.rs`。通过真实 AuthStore、HTTP router、
Engine、上传暂存、封存和提交读取持久结果，不使用模拟 Backend。夹具使用
`Config::independent_roots(Mode::Development, ...)` 和独立临时目录，覆盖一个或两个
一级盘；这些目录位于同一测试文件系统，不代表真实独立物理介质。

## 已运行的五项回归

1. Unicode、空格和 `&` 路径保存二进制正文，增加节点标签；相同正文再次保存不丢失
   标签节点、不增加正文版本；替换正文继承标签，随后增删标签。原节点标签不可变，
   旧、新正文均可通过受保护 API 完整读取，冷重开后结果保留。
2. 新建空文件、加标签、写入跨越 262144 字节校验块的二进制内容，再截短为空。
   文件身份及标签保留，冷重开后空正文和之前的非空版本均可读取。
3. 两个一级盘上的同名、同内容文件使用不同文件身份，标签互不串用，冷重开后保持。
4. 注销会话后，标签及写会话创建请求均被拒绝。重新登录、冷重开后历史、正文和列表
   保持不变；原先未执行的标签请求可以使用原操作 ID 正常执行。
5. 过期标签 HEAD 请求被拒绝且不改变历史。冷重开后，修正目标并复用未成功操作的
   ID 可以执行，仍只存在一个正文版本；再次冷重开保留结果。

第五项只比较旧节点的不可变字段。新节点成为当前 HEAD 后，旧节点响应中的
`current` 从 `true` 变为 `false` 是正常选择投影变化，不是历史被改写。

## 实际验证

- 五项真实 HTTP/Engine 测试全部通过：`5 passed; 0 failed`，测试进程约 10.45 秒。
- 聚焦测试编译通过，约 1.57 秒；严格 Clippy `-D warnings -D clippy::all` 通过。
- rustfmt、`git diff --check` 及 Cargo 测试目标接线检查通过。
- 复用唯一公共 builder 的准确默认 Engine 与 `default/core` Server rlib，不启用
  `fault-injection`，不重复构建 Engine。测试编译输出在临时内存目录，数据夹具使用
  独立工作区临时目录。单 CPU、单编译进程、2 GiB 地址空间预算下完成，编译/测试/
  Clippy 总体峰值 RSS 为 780124 KiB。
- 公共 builder 的首次 2 GiB Engine 构建因内存分配失败终止，保留失败记录；经明确
  授权的一次 3 GiB 串行重试生成了本次使用的库。此构建结果本身不算测试通过。

准确库指纹：

| 库 | SHA-256 |
| --- | --- |
| Engine | `4146f0b04f1a0aa2d9d5be71cb8597ec92434524525c861af04746afe4f3e738` |
| Server | `a7300c2484239f9e30219c6b74769353348096aa403d6f3dd1ffddaac61fd9ef` |
| Contract | `5e9a5a64767deda9955aaad6059c29c19f9e5307e517746a1a2b91c2883e39e0` |

本次以记录了所有 `--extern`、原生链接目录、源码和库摘要的 direct-rustc 命令编译
独立测试，运行 `phase1-save-tags --test-threads=1 --nocapture`，再做 metadata-only
Clippy。完整命令和日志随交付证据保存。正式 Cargo 入口是：

```sh
cargo test --locked --offline -p storage-server --test phase1_save_tags -- --test-threads=1
```

上述 Cargo 命令本轮未执行；它按现有开发依赖图启用测试配置，不能与本次复用默认
生产库的证据混为一谈。

## 未验证与产品边界

本轮未发现需要修改生产代码的保存或标签缺陷，只增加回归证据。没有运行完整
workspace、真实网络套接字、Chrome、Samba 客户端互操作或物理掉电测试。Web 仍为
元数据界面，不因此获得正文上传/编辑功能。标签范围是已有的文件节点标签接口；
目录仓库独立标签没有现成生产协议，不计为已实现。二期共享树、每文件 SQLite、
三范围元数据及复制关联均未纳入本次候选。
