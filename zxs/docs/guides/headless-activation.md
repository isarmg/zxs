# 启动 ZXS HTTP 服务

ZXS 在前台接受明文 HTTP。先按[首次部署](quickstart.md)准备空存储根和专用服务账号；本文说明监听、HTTPS 转接与停止方式。

[CI通过] [隔离客机的普通 HTTP 和 Workspace HTTPS/HTTP/2 阶段](https://github.com/isarmg/zxs/actions/runs/45322511258/job/138204176887)在 `7a88536` 通过。同一作业之后在 FUSE 阶段失败，缺盘重挂未执行。

## 监听地址

- 用 Helper 初始化 SYSTEM、DATA、回收盘和登录账户。它生成 `/zxs/state/headless/server.conf`；仍用同一个非 root 系统账号运行 ZXS。
- 明确指定数字 IP 和非零端口。IPv6 用 `[地址]:端口`；下面只在本机监听，不会自行开放其他网卡。
- 当前格式允许一块或多块 DATA；历史版单盘配置不能直接启动，也没有转换入口。不要清空或覆盖原数据来满足首设条件。

```sh
/zxs/bin/zxs --listen 127.0.0.1:8080
```

## 由你配置 HTTPS 入口

明文端口仅留在本机或可信内部网络，避免把带账号口令的请求暴露到外部。ZXS 不配置、识别、启动或监督反向代理，也不终止 TLS。

Workspace 对你选择的外部 HTTPS 地址只使用 HTTP/2。如果另设 HTTPS 入口：

- 保留原请求路径和凭据；ZXS 自行验证账户，不接受代理身份头代替登录。
- 将完整正文按准确的 `Content-Length` 转发给后端。不发分块编码、协议升级或 trailers。
- ZXS 校验完整正文和 SHA-256 后才发布节点，不依赖某一种代理产品。

## 停止与检查

发送 `SIGTERM` 后，ZXS 停止接收新连接，让已接收的请求收尾。Helper 可按所选目录和监听地址预览服务单元文本，但不安装或启用服务。

当前提交的测试范围和待验收项见[交付与验收](../../TASKS.md)。
