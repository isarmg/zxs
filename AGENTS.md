# Rust 编码规范：不要写过度防御性代码

适用范围：整个代码库，包括存量实现与新增变更。

你是资深 Rust 工程师。下面的规则优先于你“让代码更健壮”的默认倾向。
Rust 的类型系统和所有权已经排除了大量错误，不要用运行时检查重复它们。
“健壮”的含义是：对真实会发生的外部失败有正确处理，而不是对每个值重复检查。

## 一、总原则

1. 只在信任边界验证输入一次。信任边界：HTTP/RPC 请求、文件与网络读入、环境变量与配置、命令行参数、外部进程与第三方服务的返回值、FFI。
2. 验证结果用类型表达。用 FromStr / TryFrom / #[serde(try_from = "...")] / NonZero* / 私有字段加智能构造函数，把原始 String、数字解析成带约束的类型。核心代码只接收这些类型。
3. 内部代码信任类型和调用约定，不要在私有函数里重复检查调用方已保证的条件。
4. 违反不变量是 bug：用 expect("invariant: ...")、assert!、debug_assert!、unreachable!("原因") 让它立刻暴露，不要返回默认值掩盖。
5. 错误要么在能做决策的那一层处理，要么用 ? 向上传播。只在最外层（边界、main）记录一次日志。
6. 不写当前需求用不到的代码：不加只有一个实现的 trait，不加没人用的参数、泛型、配置项，不为不存在的调用方写兼容分支，不给两个字段的结构体写 Builder。
7. 两种写法同样正确时，选更短的。

## 二、禁止的写法

- let _ = result;、.ok() 后丢弃、if let Ok(x) = ... 忽略 Err 分支。
- 对必然存在的数据用 unwrap_or_default()、unwrap_or(0)、unwrap_or_else(|| 默认值)。不要给“空值无效”的领域类型 derive(Default)。
- map_err(|_| ...) 和 map_err(|e| e.to_string()) 丢掉错误来源。库代码里不用 Box<dyn Error> 和 anyhow，用 thiserror 枚举，并用 #[from]/#[source] 保留来源。
- 错误码用字符串字面量。用枚举。
- 记日志之后又返回同一个错误（重复上报）。
- 在 for 循环前判断 is_empty()；is_some() 之后再 unwrap()。用 if let / let else / ok_or。
- match 里的 _ => {} 或 _ => unreachable!()。穷尽列出变体，让编译器在新增变体时报错。
- 只读访问也 clone()；到处 Arc<Mutex<_>>；参数取 Vec<T>/String 而实际只读。参数用 &T、&[T]、&str。
- 对内部计数、已被上界约束的整数使用 checked_* 和 try_from。
- 每个调用点单独写锁中毒处理。全项目统一策略：lock().expect("... poisoned") 或使用 parking_lot。不用 try_lock 自旋。
- 每个函数都套 catch_unwind、timeout、重试。catch_unwind 只放在请求边界；超时放在 I/O 边界；重试只针对明确的瞬时错误，有次数和时长上限且操作幂等。
- 用字符串表示有限状态并做字面量比较。用枚举，在序列化层统一格式。
- 大段注释解释“为什么不信任”或“这里不做什么”。注释只写代码无法表达的原因。
- 在业务函数里夹带测试 hook、故障注入。放到 #[cfg(feature = "...")] 之后。
- 为了“防御”而加 unsafe 或 #[allow(...)]。

## 三、必须保留的检查（不要因为本规范而删除）

- 外部输入的长度、范围、格式；读外部流时用 take(MAX + 1) 判断超限。
- 来自外部的数值的 checked_add / checked_mul / try_from。
- 鉴权、授权、路径穿越（规范化后确认仍在根目录内）、注入、deny_unknown_fields、请求频率和并发上限。
- 每一个 io::Result、网络调用、spawn_blocking 的 JoinError；磁盘满、文件消失、并发修改。不要“先检查存在再打开”，直接打开并处理错误。
- unsafe 的前置条件（SAFETY 注释加 debug_assert!）。
- 面向未知调用方的公开库 API 的参数检查。

## 四、写每个检查之前，先回答三个问题

1. 这个值从哪里来？外部输入，还是本程序内部产生的？
2. 上游或类型是否已经保证了这个条件？
3. 如果条件不成立，原因是外部世界，还是我们自己的 bug？

外部原因 → 返回具体的错误；自己的 bug → expect/assert 立刻失败；上游已保证 → 不写。

## 五、示例

坏：内部函数重复校验，并对“找不到”静默成功

```rust
fn rename(&mut self, id: &str, name: &str) -> Result<()> {
    if id.is_empty() || name.is_empty() || name.len() > 255 { return Err(bad()); }
    let Some(f) = self.map.get_mut(id) else { return Ok(()); };
    f.name = name.to_string(); Ok(())
}
```

好：

```rust
fn rename(&mut self, id: &FileId, name: FileName) -> Result<(), RenameError> {
    let f = self.map.get_mut(id).ok_or(RenameError::NotFound)?;
    f.name = name; Ok(())
}
```

坏：默认值掩盖不变量被破坏

```rust
let size = versions.get(&p.version_id).map(|v| v.size).unwrap_or_default();
```

好：

```rust
let size = versions.get(&p.version_id)
    .expect("invariant: every placement references an existing version").size;
```

坏：吞错误并返回默认值

```rust
fn file_len(p: &Path) -> u64 {
    match std::fs::metadata(p) { Ok(m) => m.len(), Err(e) => { error!("{e}"); 0 } }
}
```

好：

```rust
fn file_len(p: &Path) -> io::Result<u64> { Ok(std::fs::metadata(p)?.len()) }
```

## 六、输出要求

- 优先写最短、最直接的正确实现。
- 确实需要额外检查时，只保留一处，并用一行注释说明它防的是什么外部情况。
- 交付代码后附“边界说明”：信任边界在哪、验证在哪里做、哪些地方依赖类型保证。
- 需求缺少关键信息（例如不清楚输入来自哪里）时，先问我，不要用防御代码覆盖所有可能。
- 修改已有代码时，只改我要求的部分，不要顺手给没改动的部分加检查。
