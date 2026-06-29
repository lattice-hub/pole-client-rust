---
title: SDK 模块架构
tags: [technical, architecture, sdk]
links: [pole-rust-sdk-capabilities, traffic-governance-architecture, e2e-and-public-api-testing]
updated: 2026-06-17
sources: 9
---

# SDK 模块架构

`pole_rust` 的公开入口由 `src/lib.rs` 组织，根模块包括 `discovery`、`config`、`core`、`traffic`，以及兼容旧路径的 `router`、`ratelimit`、`circuitbreaker`、`faultdetect`。

## 运行时核心

`SDKContext` 是公开 API 创建默认 SDK 的上下文入口，支持从默认配置、地址列表或完整 `Configuration` 创建。`Engine` 负责构建 tokio runtime、插件扩展、server connector、本地 cache、location provider 和 client flow，并注册 fault-detect 资源监听。

默认配置加载顺序为：

1. 当前目录 `./pole.yaml`
2. 当前目录 `./pole.yml`
3. 环境变量 `POLE_RUST_CONFIG` 指定路径

配置模型由 `global`、`consumer`、`provider`、`config` 四组配置组成，使用 camelCase YAML 反序列化，并开启未知字段拒绝。

## 公开 API 分层

- `discovery::api`：ProviderAPI、ConsumerAPI、LosslessAPI 和构造函数。
- `config::api`：ConfigFileAPI、ConfigGroupAPI 和构造函数。
- `traffic::*::api`：router、ratelimit、circuitbreaker、faultdetect、policy 的治理 API trait。
- `core::model`：命名、配置、缓存、路由、限流、熔断、错误和通用流量参数模型。
- `core::plugin` 与 `plugins`：缓存、connector、filter、loadbalance、location、lossless、ratelimit、router、stat 等插件扩展点。

## 外部依赖边界

主 crate 依赖 `pole-specification` 的官方 git tag `v0.1.0-ALPHA.26`。异步和网络层使用 tokio、tonic、hyper、h2、reqwest；本地缓存使用 dashmap；配置解析使用 serde/serde_yaml。

## 证据

- `src/lib.rs`：根模块导出。
- `src/core/context.rs`：SDKContext 创建入口。
- `src/core/engine.rs`：Engine 运行时、插件、connector、cache 和 fault-detect listener 初始化。
- `src/core/config/config.rs`：默认配置加载和 Configuration 结构。
- `src/discovery/api.rs`：服务发现公开 API。
- `src/config/api.rs`：配置中心公开 API。
- `src/traffic/mod.rs`：traffic 治理域导出。
- `Cargo.toml`：依赖、workspace 和 crate metadata。
- `tests/public_api.rs`：公开路径兼容性约束。

## 相关页面

- [[pole-rust-sdk-capabilities]]
- [[traffic-governance-architecture]]
- [[e2e-and-public-api-testing]]
