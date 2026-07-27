---
title: SDK 模块架构
tags: [technical, architecture, sdk]
links: [pole-rust-sdk-capabilities, traffic-governance-architecture, e2e-and-public-api-testing]
updated: 2026-07-18
sources: 14
---

# SDK 模块架构

`pole_rust` 的公开入口由 `src/lib.rs` 组织，根模块包括 `discovery`、`config`、`core`、`traffic`，以及兼容旧路径的 `router`、`ratelimit`、`circuitbreaker`、`faultdetect`。

## 运行时核心

`SDKContext` 是公开 API 创建默认 SDK 的上下文入口，支持从默认配置、地址列表或完整 `Configuration` 创建。`Engine` 负责构建 tokio runtime、插件扩展、server connector、本地 cache、location provider 和 client flow，并注册 fault-detect 资源监听。

默认配置加载顺序为：

1. 环境变量 `POLE_RUST_CONFIG` 指定路径（存在时优先）
2. 当前目录 `./pole.yaml`
3. 当前目录 `./pole.yml`

配置模型由 `global`、`consumer`、`provider`、`config` 四组配置组成，使用 camelCase YAML 反序列化，并开启未知字段拒绝。

`tests/data/default-config.yml` 是与当前 schema 对齐的样例契约：本地缓存位于 `consumer.localCache`，负载均衡需要 `consumer.loadBalancer.defaultPolicy`，连接器、身份、位置和配置过滤器均为启动期本地配置。熔断、限流和优雅上下线不再存在 SDK 模块级本地开关，治理效果只由控制面规则及规则状态决定。

### 治理规则生效边界

路由、泳道、限流、熔断、探测、mock、镜像和鉴权默认参与 SDK 的治理执行链。能力是否实际生效只由控制面下发规则是否存在，以及规则自身的 `enable/disable` 状态决定；SDK 不维护第二套模块级开关，也不会以远端 SDK 配置覆盖或绕过规则。

没有匹配规则时，调用链自然保持原有行为：路由返回候选实例，限流放行，熔断保持关闭状态，探测没有任务，鉴权允许，mock 和镜像没有副作用。规则发布和变更通过现有资源缓存、监听器与 API 请求时刷新链路生效。

观测语义仍在 `global.observability`，但 Collector 连接目标统一放在 `global.serverConnectors.observability`。SDK 直接连接配置的域名或 IP，不对 Collector 做服务发现二次寻址。

## 公开 API 分层

- `discovery::api`：ProviderAPI、ConsumerAPI、LosslessAPI 和构造函数。
- `config::api`：ConfigFileAPI、ConfigGroupAPI 和构造函数。
- `traffic::*::api`：router、ratelimit、circuitbreaker、faultdetect、policy 的治理 API trait。
- `core::model`：命名、配置、缓存、路由、限流、熔断、错误和通用流量参数模型。
- `core::plugin` 与 `plugins`：缓存、connector、filter、loadbalance、location、lossless、ratelimit、router、stat 等插件扩展点。

## 外部依赖边界

主 crate 依赖 `pole-specification` 的官方 git tag `v0.1.0-ALPHA.33`。异步和网络层使用 tokio、tonic、hyper、h2、reqwest；本地缓存使用 dashmap；配置解析使用 serde/serde_yaml。`GLOBAL` QPS 限流按规则中的 `RateLimit.cluster` 懒加载限流服务 gRPC 流；`LOCAL` 与并发数规则保持本地计数。

## 证据

- `src/lib.rs`：根模块导出。
- `src/core/context.rs`：SDKContext 创建入口。
- `src/core/engine.rs`：Engine 运行时、插件、connector、cache 和 fault-detect listener 初始化。
- `src/core/config/config.rs`：默认配置加载和 Configuration 结构。
- `src/core/config/global.rs`：全局静态配置和观测配置 schema。
- `src/core/context.rs`：SDKContext 静态创建入口。
- `src/discovery/api.rs`：服务发现公开 API。
- `src/config/api.rs`：配置中心公开 API。
- `src/traffic/mod.rs`：traffic 治理域导出。
- `e2e_tests/src/config.rs`、`e2e_tests/src/cases.rs`：仅依赖 console/client 地址的 e2e bootstrap 与治理规则流程。
- `Cargo.toml`：依赖、workspace 和 crate metadata。
- `tests/public_api.rs`：公开路径兼容性约束。

## 相关页面

- [[pole-rust-sdk-capabilities]]
- [[traffic-governance-architecture]]
- [[e2e-and-public-api-testing]]
