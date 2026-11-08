# 辅助工具独立 Web：只读首片

[辅助工具](../components/helper.md) · [职责迁移](helper-disk-management.md) · [任务清单](../../TASKS.md)

本片实现独立 HTTP 路由和内嵌页面，不需要先启动 ZXS，不是在线管理 API 的代理。后继另加入[固定路径的程序首装](helper-program-installation.md)；来源盘接口仍只读，不能把程序安装当完整系统初始化。**当前仅为新逐盘协议的只读检查和核验下载，不是完整辅助控制台已经交付。** 新代码按开发期最新要求未构建、未运行测试、未运行真实浏览器或 CI。

## 手动启动及网络边界

```sh
sudo ./zxs-helper
# 等价显式命令；可更换监听地址和端口
sudo ./zxs-helper serve --listen 0.0.0.0:9040
```

无子命令时进入控制台，默认监听所有 IPv4 接口的 9040 端口；控制台入口要求有效 UID 为 root，拒绝后不创建运行目录、不安装或启动服务。既有显式 CLI 命令仍使用各自原有权限准入，不因为新增 Web 而绕过目录所有权检查。

从浏览器直接打开 `http://主机IP:9040`，无需登录。使用 IP 字面量或本机 `localhost`，Host 必须匹配实际监听端口；指定监听 IP 时也必须匹配该 IP。拒绝任意 DNS Host，避免将浏览器 DNS 重绑定视为同源；不信任 Forwarded/X-Forwarded-*，本片不提供反向代理配置。IPv6 监听可以显式指定，但本片没有实际 IPv6 验收。

启动日志和页面同时提示：**无认证、root、明文 HTTP，网络可达者可能读取所选盘数据或关闭辅助工具**。仅用于可信隔离网络／防火墙，使用结束后退出。没有修改防火墙、注册 helper systemd 服务或启用自启的代码。CSRF 和 Host 检查不等于用户认证、TLS 或安全公网部署。

## 真实接口与输入

所有资源内置二进制：`helper/web/{index.html,console.js,console.css}`。页面不加载 CDN、不使用浏览器持久存储、不打开在线 Engine、不扫描或自动登记未知盘。

| 路径 | 当前行为 |
| --- | --- |
| `GET /`、`/console.js`、`/console.css` | 独立页面与资源，不要求会话或账号。 |
| `GET /api/status` | 返回进程模式、版本、忙碌／退出状态及每进程 CSRF 值；明确 `service_exclusion_established=false`、`source_writes_available=false` 和未实现功能。 |
| `POST /api/authority/check` | 输入 `{selection, writers_stopped}`。直接复用 helper 内的检查报告代码及 Engine 通用 `ContentSnapshot`，返回完整元数据快照中的逐正文状态，不写来源盘或数据库。 |
| `POST /api/authority/export` | 输入 `{selection, writers_stopped, commit}`。重新打开准确来源并核对已知检查点仍在其发布链中、完整核验所选提交和正文后才返回二进制；不在服务器创建输出文件。 |
| `POST /api/shutdown` | 只接受 `{confirmation:"stop-helper"}`，关闭新工作准入并请求真实排空，不停止 ZXS、不注销或移除磁盘。 |

`selection` 与 `authority` CLI 的当前 `DiskSelection` 相同：绝对磁盘路径、完整原来源身份与 incarnation、可选已知检查点。不得用目录名、系统库或未知盘自动推测身份；不转换在线池格式。浏览器可读取操作者手选的本地 JSON 文件并显示在表单中，只有明确提交后才发给辅助工具。

所有 POST 要求单一准确 Origin 与 Host、同源 Fetch Metadata（存在时）、单一 CSRF 请求头和准确 `application/json`。CSRF 值从本进程的直接可读状态接口取得，不是登录凭据，不授予独立于网络可达性的认证权限。无 CORS 响应，拒绝 URL 查询参数、未知字段及非同源请求。响应使用 `no-store`、`nosniff`、`DENY` 和只允许自身资源的 CSP，未把任意来源数据插入 HTML。

POST 输入最大 64 KiB，正文读取期限 15 秒；磁盘读取只允许一个正在接收或执行的请求，不排队堆积。JSON 边界异常保留具体来源。元数据、身份、预算或文件系统错误没有“健康”报告；正文缺失／损坏在完成的检查报告中明确区分。检查报告为 `content_issues` 时仍是成功取得报告，不等于健康，CLI 原有非零退出语义保持。

## 下载承诺

页面只为本次检查为 `verified` 的项提供下载；服务端不相信这份先前观察，仍重新验证指定版本。完成后响应 `application/octet-stream`，文件名仅由已验证提交摘要构造，并附带 `X-ZXS-Commit`、`X-ZXS-Body-Digest`、`X-ZXS-Checkpoint`。浏览器确认响应提交／正文摘要和字节数与所选版本一致后才请求保存；可另存来源和版本核验信息。

检查点是已知发布链锚点，不要求当前头没有新增合法后继；导出仍绑定所选不可变提交与正文，不把更晚检查点当成当前全局最新性证明。

正文摘要使用既有 ZXS `body` 域算法，**不是 SHA-256**。校验信息只是一份观察，不能替代来源权限、在线发布回执、全系统恢复证明或正式接纳。网络是明文且未认证，不能据响应头宣称防止中间人篡改。正文不存在、损坏或无法重核时只返回具体错误，不返回未验证正文，不借缓存补造成功。

## 所有权、退出与未完成范围

操作者必须先停止所有源盘写入者，再勾选确认；这仅为操作者确认，**不是已获得主服务跨进程停服锁**。本片只有只读快照能力，没有写者、初始化、注册、发布或修复入口，因此不把确认字段用作未来写操作的准入替代。root 不自动获得 Engine 的其它 UID 私有目录能力，当前通用安全文件系统仍要求其原所有权／权限；不 chmod/chown 或绕过这些限制。

磁盘工作进入独立阻塞执行者，并由执行者持有唯一许可，浏览器断连不会提前释放。SIGINT、SIGTERM 或退出按钮停止新准入，等待正在处理的 HTTP 请求与阻塞磁盘读取真正结束；慢请求的正文读取仍有上述期限。运行时退出继续 drain 阻塞线程，不把 I/O 永久阻塞或 OS 强制结束伪装成安全退出。页面关掉不代表进程退出；先等终端确认退出再移除媒体或启动写入者。

首次初始化、磁盘添加／注销、完整低级链外来接纳、在线池初始化、三范围系统重建、密码重置、安装／修复及内置 ZXS 载荷仍未实现。既有在线安全入口本片不删除；没有磁盘计划下线按钮。将写操作接入 Web 前必须完成共同服务互斥、root／服务账户权限交接、持久操作 owner 与中断恢复，不能复用本片的布尔确认充数。

对应测试源码在 `test/helper/unit/console/tests.rs`：直接开放页面、请求来源／头边界、准确盘的检查与下载、不写来源、拒绝错误正文／身份、单任务、退出和输入上限。此处列的是测试源码，不代表已运行或真实 TCP／浏览器／信号排空验收。

## 后继部署准备

独立控制台新增 `/api/deployment/preview`、`/api/deployment/prepare`，按完整输入生成和保存[未激活 systemd/SMB 配置包](helper-deployment-preparation.md)。该事务复用同一 CSRF/Origin、单任务 owner 与排空，固定写 `/zxs` 的新专用目录，不替换现用服务配置，未取得主服务停服租约也不宣称服务已启用。源码未运行。

## 根目录准备与当前 daemon 租约

后继 `/api/host/preview`、`/api/host/prepare` 提供 root 明确确认的固定 `/zxs` 空根准备，不能当完整初始化。部署包准备现在与当前 ZXS 二进制共享同一目录 inode 租约，空闲控制台不持锁，也不宣称 Samba/旧二进制已停。见[生命周期边界](helper-service-exclusion.md)。源码未运行。

## 首次初始化的私有布局前置

`/api/initialization/layout-preview` 和 `/api/initialization/layout-prepare` 使用准确本地账户与相同作业 owner/CSRF/Origin，持有当前 daemon 生命周期租约，固定创建尚不存在的空私有数据树。页面明确不创建管理员、数据库或磁盘，不将此回执称作初始化完成。详见[布局边界](helper-private-data-layout.md)，源码未运行。

### 固定系统配置文件安装后继

控制台另有 `/api/deployment/install-preview` 与 `/api/deployment/install-files`，使用同一单工作 owner、同源/CSRF/Host 与退出排空。真实目标仅 root 所有的固定 systemd unit 和独立 Samba 配置；确认文案明确原请求、无覆盖、外部管理须停止与不加载/启动/启用。网络风险提示同步扩大到这些写权限，没有登录或秘密。源码未运行，详见[固定安装边界](helper-configuration-file-installation.md)。

### 已存在管理员的密码重置后继

`/api/credentials/reset-preview` 和 `/api/credentials/reset` 沿用独立控制台单工作 owner、同源/CSRF/IP Host 和退出排空。重置请求秘密字段不进入日志、URL、argv 或回执，JSON 错误不展开可能引用私有字段的细节；新密码由用户手动输入，失败后需同一原请求与同一新密码明确续作。无认证/明文 HTTP 的风险提示明确可达者也可能改密。它不初始化缺失账户，不操作 Samba 密码或 Engine 灾备副本。源码未运行，见[重置协议](helper-administrator-password-reset.md)。

### 首次管理账户后继

`/api/initialization/administrator-preview`、`administrator-create` 和 `administrator-recover-preview` 将新空布局内的管理账户创建和同原请求续作接入 Web；手动秘密输入与已有管理员重置共用受保护 HTTP 解码，不新增登录、URL/argv 密码或自启。仅形成 management-without-storage，完整首批磁盘/数据库/干净启动交接仍缺；源码未运行，见[准确边界](helper-first-management-administrator.md)。

## 首批存储工作进程

首批空目录预览/初始化/原计划恢复已接 `storage-preview`、`storage-create`、`storage-recover-preview`，放在首次管理员之后。使用受限同二进制 service UID/GID 子进程，不放宽 root 对 Engine 的权限规则；父任务等待子进程结束，响应丢失保留原请求。当前仍无登录、默认 0.0.0.0、手动 root，无 helper 自启。入口和不代表完整首次安装的边界见[首批存储](helper-first-storage-batch.md)；源码尚未编译/测试/审查或运行 CI。
