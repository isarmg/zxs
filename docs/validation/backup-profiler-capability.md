# Backup 外部工具能力探测

## 动机与已验证范围

原完整事务仍在 900 秒超时。#1312 的准确提交为
`d1a8ad179271355f4e0cb9ec509214ac19f393ea`，完整相关比较基线为
`24a18efc4adc50c9a1c432032dd5df093154863f`。
本轮 292 项 Engine 与 38 项 Platform，共 330 项 Rust，
8 个 Python 模块共 176 项测试及 1 个 Engine 无默认特性产品检查通过。
包含七类真实原生归档的 owned/borrowed 解码差分、旧格式拒绝与当前格式冷开，
两项 pending 夹具修复和全部 21 项 readmission（含新增两项恢复准入回归）。

原完整事务是唯一失败，仍为精确原用例的 900 秒超时；
各执行片 selected skipped/blocked 均为空。组件测试为 53.27 秒，
不能与 #1308 的 60.61 秒或 #1311 的 88.28 秒跨 runner 比较为优化收益。
同次原事务约 866 秒才观察到 SYSTEM 安装阶段，native metadata 未报告完成。
这不证明三范围共同恢复、后继发布或完整 ordinary-serving 已闭环。

工件：
- Engine/CI：
  https://github.com/isarmg/zxs/actions/runs/43249410350/artifacts/13960585980
  SHA256 `f16e261b0a5bba57bb8e7e55b698540058d173df672cdfda697f9739d1a8c2c8`
- Platform：
  https://github.com/isarmg/zxs/actions/runs/43249410350/artifacts/13958872740
  SHA256 `4cc6e91610ed872dbbe0e3a27a66854b7cd0e1ede5fe79ab73ce6ddc4d1cc682`
- 原完整事务：
  https://github.com/isarmg/zxs/actions/runs/43249410350/artifacts/13960865548
  SHA256 `8e7560359abfc9c11832d4f29595bd47501a67c9d836a2a742fb0148e851a898`

静态调用图确认一次成功 SYSTEM observation 可经多层前后验证递归进入
serving observation 32 次（2×2×1×2×2×2）。这些调用夹住实际读盘，
现有 Report 不持有全部读回字节的存续证明，不能直接复用旧 Report 或
压平调用以跳过验证。现有 /proc 证据只支持持续 CPU/逻辑读取，不是热函数栈。

## 本片只验证工具能否启动固定探测

新增独立手动 workflow `Backup profiler capability`：
只在明确 workflow_dispatch 时运行，不增加 push/schedule/PR 自动入口，
不改变原 CI、分片覆盖、原 900 秒测试、退出码解析或失败聚合。
检出和报告绑定同一 GitHub SHA，权限仅 contents:read，action 使用现有固定 SHA，
检出不保存凭据；总 job 上限 2 分钟，只保留一份能力 JSON。

探测只使用 runner 已安装的 perf/GDB，固定系统 true/false 子进程，
不编译或执行仓库业务、不附加已有 PID、不安装工具、不提权，
也不修改 perf/ptrace/系统安全设置。显式保留 ASLR，
关闭 GDB 初始化文件、自动加载、debuginfod 和启动 shell。
子进程环境仅固定必要值，不继承或输出令牌、用户环境或业务参数。

最多启动六次子进程：两次版本查询、每工具两次 true/false。
版本上限 2 秒，单次功能探测 5 秒，共用 40 秒绝对截止时间；
失败清理有单独短上限。每次捕获输出最多保留 64 KiB，允许额外一字节检测超限。
这是输出保留限额，不是总内存上限；解析和 bytes 转换会产生有界副本。
原始工具输出只留在进程内存，报告仅含固定状态、数值版本、退出码及安全分类，
不上传 raw MI、路径、argv、环境、线程名称或寄存器/局部变量。

## 失败与清理边界

工具未安装或明确权限拒绝可以形成有效的 unavailable 诊断；
不能因为工具存在、内核 paranoid 数值或 CLI 返回 0 就宣称能力可用。
perf 还须提供实际事件计数，GDB 还须提供与固定 true/false 对应的退出事件。
timeout、输出超限、内部错误、全局截止或清理未确认必须使探测失败，
停止继续启动并非零结束，仍尽可能保留失败报告。

只清理自身直接子进程及经 GDB MI 明示、/proc 父子和启动身份、
可执行文件 inode 与 pidfd 复核的已知 inferior。不扫描已有进程，
不声称可清理逃逸后代。perf 无 MI、GDB 尚未取得 pidfd 的超时窗口，
必须明确报告 inferior 清理未证，不能冒充已清理或据此准入正式测试采样。

## 不能由本片推断的结论

perf stat 成功只证明当前固定事件计数可用，不证明 perf record 的函数采样权限。
GDB true/false 成功只证明这两个固定进程的启动/退出协议，不证明真实 Rust 符号、
有界 interrupt/backtrace、异常信号、原 stdout/stderr 分离或完整子进程树清理。
这些都须另行证明，不能把调试器直接替换进原测试。

本片的 GitHub Python 回归使用受控 mock 验证资源/退出/路由契约，
不是实际工具权限证据。手动探测 workflow 即使成功，也只代表该诊断已完成，
不代表原 Backup 事务通过。当前尚未执行本片 GitHub 回归或手动工具探测；
未本地执行脚本、编译、测试、格式化或 selector。
