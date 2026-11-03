<a id="service-stack"></a>

# 服务组合层

`service-stack` 提供 `Service`、`Layer`、层组合、服务适配器及可选中间件。

## 请求服务的调用契约

`service_stack::Service<Request>` 通过 `poll_ready(&mut self, cx)` 表示能否接受请求，通过 `call(&mut self, request)` 返回响应 future。调用者必须先等待 ready；未就绪就调用可能 panic，也会破坏限流/缓冲的背压。ready 所预留的容量属于该服务实例，不应在 ready 之后换成它的 clone 再调用。

启用 `util` 可使用 `service_fn`、`ServiceExt::ready`、`oneshot` 等工具。`ready().await?.call(request)` 保留服务供多次使用；`oneshot(request)` 消耗服务，内部先等待 ready，再执行一次请求。

```rust
use std::convert::Infallible;
use service_stack::{service_fn, Service, ServiceBuilder, ServiceExt};

async fn service_example() -> Result<(), Infallible> {
    let mut service = ServiceBuilder::new()
        .map_response(|value: u32| value + 1)
        .service(service_fn(|value: u32| async move {
            Ok::<_, Infallible>(value * 2)
        }));
    assert_eq!(service.ready().await?.call(3).await?, 7);
    Ok(())
}
```

## 层顺序、超时与 HTTP

`Layer<S>` 负责从内层服务构造外层服务，不执行请求。`ServiceBuilder::new().layer(a).layer(b).service(inner)` 的请求顺序是 a → b → inner，响应反向返回。不要套用到连续的 `Router::layer(a).layer(b)`：后者的 b 包在外面，先看到请求。

顺序会改变资源预算。例如 `buffer(100).concurrency_limit(10)` 允许最多 100 个排队请求加 10 个处理中请求；反过来先限并发，则入口总共只放行 10 个。选择这些中间件须开启相应 `buffer` / `limit` 特性，不要把队列长度当成全服务的统一并发上限。`ConcurrencyLimit` 本身只使用信号量，不要求活动的 Tokio 运行时；`ServiceBuilder::buffer` 通过 `Buffer::new` 启动 Tokio worker，因此需要运行时。需要自行驱动 worker 时可使用 `Buffer::pair` 并交给其他执行器；基于时间的 `RateLimit` 和 `timeout` 则需要 Tokio 时间驱动。

`timeout` 计时从 `call` 开始，取消被包装的响应 future；它不会自动覆盖进入该层 `call` 之前的就绪或排队等待，也不保证响应正文流已经全部传完或服务端副作用回滚。`load-shed` 将未就绪转为立即错误，`retry` 必须由调用者提供适合业务的重试策略，尤其不能盲目重试非幂等写操作。

接入 [http-routing](http-routing.md) 时，可能失败的中间件需要在外层用 `HandleErrorLayer` 等机制转成 HTTP 响应，不能让服务错误漏到连接层。直接接入 [http-transport](http-transport.md) 时用 `TowerToHyperService` 适配，不能把两个不同的 `Service` trait 当成同一个接口。

## 随机源的装箱委托

`util::rng::Rng` 的 `Box<R>` 实现转发全部三个方法：`next_u64`、`next_f64` 和 `next_range`。具体类型、trait object 及嵌套装箱均保留实现者的覆盖方法；仅实现 `next_u64` 的随机源继续使用 trait 的默认浮点和范围算法。

`Balance::from_rng` 会将随机源装箱。启用 `balance` 时，双候选抽样仍通过传入随机源的 `next_range` 选择两个不同候选，不因装箱改用默认算法。本次修正只恢复委托，不改变默认分布、范围约束或随机源强度。

<a id="boundaries"></a>

## 能力边界

- 无特性时没有外部依赖；`Service`、`Layer`、`ServiceBuilder`、恒等层、堆叠层和函数层支持原生及浏览器 WASM。
- `util`、`filter`、`load-shed`、`steer` 为显式启用的纯服务工具，不引入 Tokio。
- 运行时中间件按需启用；`full` 是维护范围内的原生中间件并集，不是默认值。
- 全库禁止 unsafe，不负责 HTTP 协议解析或 TLS。
- 不提供 `hedge` 中间件或对应特性，`full` 也不包含它。

<a id="validation-and-origins"></a>

## 检查与来源

运行 `python3 .github/scripts/check-service-stack.py`，检查原生/WASM 特性隔离、零依赖基础 trait，以及真实层组合、错误、取消和 trait 身份。随机源回归在 `util` 下执行 6 项，在原生 `full` 下执行同样 6 项及 1 项真实 `Balance::from_rng` 委托回归。依赖不可用 `tower-test` 的上游用例及 hedge 测试仅保留来源，不计为执行。

来源为 [Tower 0.5.3](../upstream/tower-0.5.3/README.md)、[tower-layer 0.3.3](../upstream/tower-layer-0.3.3/README.md) 和 [tower-service 0.3.3](../upstream/tower-service-0.3.3/README.md)。当前导入统一使用 `service_stack`；原始文档仍按上游身份保留。精确源码、许可证、上游版本和测试归属保存在 `compliance/provenance`、来源目录及整合凭据中，包括不启用的 hedge 原始生产文件。
