# Rust 契约校验器

`contract-check` 是根工作区内的契约检查包，复用本项目 `storage-contract` 的严格 JSON、整数编码、JCS、BLAKE3 域分离和校验清单解码。没有并行维护的其他语言规则。

```sh
CARGO_INCREMENTAL=0 cargo run -p contract-check \
  --locked --offline --target-dir target/native
CARGO_INCREMENTAL=0 cargo test -p contract-check \
  --locked --offline --target-dir target/native
```

也可传入 `-- --root /absolute/path/to/zxs` 检查指定副本。成功输出 JSON 范围报告，失败退出码为 1。Rust 依赖源码由仓库提供，首次构建同样使用唯一根锁文件及 `--locked --offline`，无需预热 registry 缓存；Rust 工具链和必要系统构建工具须预先准备。

检查内容：

- 两份 JSON Schema 的 2020-12 元 schema 有效性及 OpenAPI 3.1 官方结构约束。
- OpenAPI 本地引用、路径参数、重复 operationId/参数和安全方案引用；与核心 schema 的内嵌定义完全一致。
- 定向契约测试覆盖文件版本真实父版本数组（根、去重、多父及必填）、文件、版本及节点备注的128字节无控制字符写入规则，以及当前合法备注和非字符串拒绝的读取字段形状、监测实时/历史共用状态结构、功耗缺失值、管理员鉴权声明和361点历史上限。
- 分支记录和不可变快照的 pending 映射 ID、文件作用域、快照条目一致性，旧 Branch/Commit 省略空映射后的规范字节及摘要兼容；文件管理继承投影区分可见父版本与自有版本，父状态独立表达，真实零字节 FULL 和目录空字符串仍可区分。
- 文件合并、标签、作用域备份与备份命名空间的真实响应使用版本管理中的[固定 fixture](contract-check/tests/fixtures/merge-tags-backup.md)校验完整 OpenAPI 响应 schema；干净源码副本无需预先生成本地 evidence。
- 所有正反字段向量和语义拒绝向量，覆盖路由、副本成员、发布证明、请求摘要、上传完整性和资源配额关系。
- 操作结果核对与显式健康诊断分别使用严格 outcome/observation schema；真实客户端校验分别消费两种响应，不因当前健康变坏重写历史发布证据。pending 的 NotObserved/null 与 Checked 对象有独立正反向量；原持久化回执及规范摘要向量保持不变。
- 固定 JCS 字节与 BLAKE3、严格 JSON 拒绝向量、空文件/短文件校验清单，以及截断和损坏变体。
- 文档中目标 TOML 配置的受检投影与 schema/向量一致；分支必须只登记冻结来源、首写前继承父内容，拒绝创建即全量物化或空内容占位。该投影不是当前进程的 JSON Config。
- 根 README 与 `docs/` 下 Markdown 的本地链接、显式锚点去重及 JSON/TOML 示例语法；跳过生成的发行目录。`test/evidence/` 中的本地生成附件单独统计，源码副本缺少附件时记录缺失数量；源码、文档、固定样例、schema 及检查脚本的链接仍严格要求存在。

`cargo test` 还在独立临时 Git 仓库中验证根 `.gitignore`：检查构建缓存、运行数据、本地配置、验证 JSON/截图/日志被排除，同时源码、锁文件、字体、固定样例、测试程序和验收说明保持可见。该测试需要 Git，不读取或修改项目索引和个人 Git 忽略配置；普通契约校验命令不依赖 Git。`local_generated_artifact_links` 和 `missing_local_generated_artifacts` 分别表示生成附件链接数与未在当前副本中生成的数量，不表示这些附件通过验收。

校验器不启动服务或访问数据盘，不能证明协议端点、三盘持久性、模型或真实平台已经验收。HTTP/文件引用解析特性在 `jsonschema` 依赖中关闭；OpenAPI 包内只允许可解析的本地 `$ref`。

检查数量由当前输入决定，不固定旧报告计数。

依赖来源与许可：

- 固定的 [OpenAPI 3.1 元 schema](../../tools/contract-check/schema/openapi-3.1-2022-10-07.json) 来自 <https://spec.openapis.org/oas/3.1/schema/2022-10-07>，SHA-256 为 `da01ba28852cac0de53893797cb8d1942bc3b05084f526dcc216717dec314ed0`；上游许可全文集中在下方，来源为 <https://github.com/OAI/OpenAPI-Specification/tree/3.1.0>。
- `jsonschema` 来自 <https://github.com/Stranger6667/jsonschema>；其他依赖使用 Cargo.lock 中的原包名、来源和校验值。

## OpenAPI 许可全文

```text
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "{}"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright The Linux Foundation

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
```
