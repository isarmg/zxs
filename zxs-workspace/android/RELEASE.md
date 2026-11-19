# Android 版本化 APK 候选

这是现有只读查看器的**本地工件交接**，不是下载链接、签名服务或应用商店发布。应用 ID 为 `org.zipworkspace.preview`；当前 `versionName=0.1`、`versionCode=1`。版本只在本目录的 `build.gradle` 维护；对外升级时保持正确应用 ID 和签名身份，按 [Android 版本规则](https://developer.android.com/studio/publish/versioning) 提高版本码。

## 从独立源码准备工件

以下命令供有授权的所有者在干净环境中**以后**操作。本目录本身就是 Gradle 根，无需其他项目，无 Gradle Wrapper 或额外打包插件。先备好并核实 JDK 17、[Gradle 8.13](https://gradle.org/release-checksums/)、[AGP 8.13.2 / SDK Platform 36 / Build Tools 35.0.0](https://developer.android.com/build/releases/agp-8-13-0-release-notes#compatibility)。平台 API 36 与 Build Tools 35.0.0 是不同组件。使用普通账户、干净且尚无 `build/` 输出、不会被并发改写的可信工作副本，预置经过核对的官方 SDK 和专用 Gradle 离线缓存；无自定义 init 脚本、IDE 注入参数或签名材料。缺 SDK 或依赖就停，不在线补装或自动接受协议。严格隔离网络时请另用断网的构建环境；`--offline` 不是第三方代码的网络沙箱。

在 `android/` 下运行，指定一个**尚不存在**的完整目录；它的父目录须事先存在、由操作者独占、不被同步或移动，且在工程之外：

```sh
gradle --offline --no-daemon --no-build-cache --no-configuration-cache \
  -Pandroid.builder.sdkDownload=false \
  -PworkspaceApkHandoff="$HOME/zip-workspace-apk-candidate-01" exportReleaseApk
```

当前版本完成后，指定目录应仅有：

- `zip-workspace-android-0.1-vc1-release-candidate.apk`：AGP 本次 `release` 变体的单个未拆分产物，不取 debug、测试或其他变体，也不在旧输出中搜索文件名。
- `candidate.json`：最后写入的交接记录，含应用 ID、变体、两个版本字段、文件名、字节数和复制时的 SHA-256。它记录来自 [AGP 产物元数据](https://developer.android.com/reference/tools/gradle-api/8.13/com/android/build/api/variant/BuiltArtifactsLoader) 的声明和这一份文件的散列，不是签名、APK 内部清单证明或安装许可。

已有输出不覆盖；失败或中断可能留下部分 APK/记录，不要使用它们，也不自动删除。只有任务成功且外部复核文件和记录一致，才交给所有者。本任务不读密钥、不配置 release 签名、不安装或上传；普通 `assembleRelease` 不会顺带交接工件。只把它称作“按无签名配置构建的待处理候选”：**不能**把任意 `apksigner verify` 失败当成字节一定无签名，损坏文件也可能失败。

## 准备签名、交付前的核对

1. 在同一授权环境重新计算候选 SHA-256 和长度，与 `candidate.json` 对照；记录确切源提交及工具版本。用官方 [apkanalyzer](https://developer.android.com/tools/apkanalyzer) 的 `manifest application-id`、`version-code`、`version-name`、`min-sdk`、`target-sdk`、`debuggable`、`permissions` 和 `manifest print` 查看 APK 实物。当前应为上述 ID、1、0.1、35、36、不可调试；不得出现 testOnly、网络或新增的广域存储权限。对不明来源、非预期签名或损坏产物停下，不要以重签替代调查。
2. 使用 SDK 的 `zipalign -c -P 16 -v 4 <候选.apk>` **只检查**现有对齐；不通过就重新调查。获授权的所有者在自己的签名系统中把候选签为另一个新文件，不在源码、Gradle 属性、聊天或命令行参数中保存密码。签名是单独环节，不由本任务调用。用 [apksigner](https://developer.android.com/tools/apksigner) 的 `verify --verbose --print-certs <已签.apk>` 验真实已签新件，并将实际签名者证书 SHA-256 与所有者**独立确认**的预期身份比对；不要仅因“有个签名”就放行。签名后不得再改动包；另算已签新文件的 SHA-256，不能沿用候选摘要。参见 [对齐与签名的顺序](https://developer.android.com/tools/zipalign)、[Android 签名与更新](https://developer.android.com/studio/publish/app-signing)。
3. 在干净环境验：无缓存/SDK 时离线失败；签名或 IDE 注入在 APK 生产前拒绝；已有交接目录/多份或拆分产物不作交付；调试和普通构建不写交接目录；已有相同名称或应用 ID 不等于可升级。用 Android 15+ 真机分别验全新安装、同签升级及本应用的选档、取消、CRC 和纯文本边界。最终核定本产品和实际依赖的分发权、保留许可与真实实机结果，再决定是否对外发布。此流程不生成 AAB、不上传 Play，也不代表已有可安装工件。

本次仅提交源码和步骤。没有构建 APK、运行仪测、读取签名凭据、签名或连接任何设备。
