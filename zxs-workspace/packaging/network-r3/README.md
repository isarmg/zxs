# Workspace R3 HTTPS 候选依赖

该清单是提交传输的独立打包输入，不修改已有发行配方，不会安装软件或启用连接。六个包均有官方 PyPI `py3-none-any` wheel，无新增本机扩展。HTTPS 使用 CPython `ssl`、httpcore 的单连接及标准 ALPN，优先协商 HTTP/2；端点只选 HTTP/1.1 时可回退，不因请求断线而换版本重发。公开 CA 使用签发机构信任包，私有部署可在同一配置中选定自己的 CA；仍验证证书链和端点名称，不固定叶证书。

- Windows / macOS：经单独审查后，将这六条加入现有各自的哈希锁定 wheel 清单，并在冻结候选中核对实际包含 `httpcore`、`h2`、`h11`、`hpack`、`hyperframe`、`certifi`、CA 数据及平台自身 `ssl` / OpenSSL 依赖。旧 PyInstaller 最小闭包和许可清单一起更新，不靠系统额外安装的包或环境变量。只在启用真实客户端的发版候选增加依赖，不让仅本机工具拉网络。
- Ubuntu：现行 deb 配方只认已核输入，不从 PyPI 自动取包。若所选 Ubuntu 源能提供与固定适配接口相符的 `python3-httpcore` / `python3-h2`，走系统打包依赖，另外核实 h11、hpack、hyperframe、certifi 和 CA 依赖；若版本和行为不符，保持网络功能关闭，或另审同一锁定 wheel 的私有入包方案。不要把未经核对的系统版本当成等同。
- 三个平台实际发布前均要分别验打包程序正常导入及存在真实 CA 信任、ALPN H2 / H1 回退、错误主机名或不受信任的证书、断线后的不重发、完整封存正文分段发送和原操作 GET 回执。不把源码或元数据核对当成实机结果。

依据：[httpcore 1.0.9 的公开网络后端](https://www.encode.io/httpcore/network-backends/)、[精确连接实现](https://github.com/encode/httpcore/blob/1.0.9/httpcore/_sync/connection.py)、[PyPI httpcore](https://pypi.org/project/httpcore/1.0.9/#files)、[PyPI h2](https://pypi.org/project/h2/4.3.0/#files)。没有下载、构建、安装或验收工件。
