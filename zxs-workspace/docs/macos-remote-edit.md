# macOS 远程节点：本机单文件编辑与封存

这是供未来虚拟盘使用的内部数据读写接口，没有 Finder、CLI 或挂载入口。它不创建可见的下载文件，不改远端，也不经过独立 ZIP 工作区。

`macos_edit_file.selected_edit_file` 需要原已认证连接、选中的容器／分支／节点，以及操作者控制的现有 0700 本机 APFS 私有目录。该目录须没有扩展 ACL、无同步或其他程序同时更改。下载前后重新向同一连接核对容器版本、原分支头和所选节点。可以选分支头或历史 main 节点，不跟随后来出现的头。只从确切节点的 HTTP/2 GET 接收正文，核长度、SHA-256，再完整读回本机字节。

原文件名不参与本机文件创建。只独占创建随机名的 0600 空文件，在实际 APFS 上打开同一实体的独立读写和只读句柄，核实后在原父目录中[移去空暂存名](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/unlink.2)，同步父目录，并确认链接数为零，之后才接收正文。若准备中断，可能留下空暂存名；不用名字清理。

同一个本机匿名文件会话可有多个读者，只能有一个写者；这不是跨会话或跨客户端的写锁；读、写、截断及同步都使用在手的原文件。写请求不超过 16 MiB，单文件上限 128 GiB。停接新句柄后，原句柄仍可完成最后一次写入再关闭。写入或同步出现错误时不能封存；关闭、普通保存、同步和停写都不生成远端版本。只有另行明确请求、该会话所有句柄均已释放，才会同步并关闭最后一个可写系统句柄，从原只读句柄计算完整长度和 SHA-256。后续只能在原会话存活时按块取这份封存，断开认证则停止。

**边界：** 本机封存不是持久提交票据，退出进程会失去未移交的修改。尚无目录与大小写别名展示、系统卷挂载、编辑器临时文件替换、原卷正常卸载、持久待提交队列或网络提交的接线。正式接入前须证明内核端已停止接收新写入且缓存和在途写入均已排空，不能把最后一次关闭当作该证明。[macFUSE 5.5.0 的 FSKit 限制](https://github.com/macfuse/macfuse/releases/tag/macfuse-5.5.0)和[文件系统同步的保证](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/fsync.2)应分别验证。

这个匿名编辑文件不替代「另存」。公开可见的 macOS 下载仍需系统隔离标记并读回与实际请求完全一致的 [Launch Services 下载 URL](https://developer.apple.com/documentation/coreservices/klsquarantinedataurlkey)。已观察到隔离类型正确但系统未返回下载 URL；这与 [Chromium 记录的系统行为](https://chromium.googlesource.com/chromium/src/+/e1c0d01fecb19602388a71798c63c15f69d13f55/components/services/quarantine/quarantine_mac.mm)一致。不能用设置调用成功、Finder 来源文本或应用自写属性代替，入口仍关闭。

[合成资格用例](../tests/qualification/test_macos_edit_file.py)覆盖原生匿名句柄、单写者、停写后排空、显式整文件封存、空文件、不提交即关闭、短写／同步失败、来源变化和断开；未在 macOS 运行。
