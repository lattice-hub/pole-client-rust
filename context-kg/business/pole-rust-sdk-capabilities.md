---
title: Pole Rust SDK 能力概览
tags: [business, sdk, capability]
links: [sdk-module-architecture, traffic-governance-architecture, e2e-and-public-api-testing]
updated: 2026-06-17
sources: 7
---

# Pole Rust SDK 能力概览

`pole_rust` 是面向 Pole 的 Rust SDK，Cargo metadata 将其描述为轻量级 Proxyless Service Governance SDK。当前公开能力覆盖服务发现、配置中心、服务治理和外部环境 e2e 验证。

## 核心能力

- 服务发现：Provider 侧支持实例注册、反注册、心跳和服务契约上报；Consumer 侧支持获取单个实例、健康实例、全部实例、监听实例变化、获取服务规则和上报服务调用结果。
- 配置中心：支持配置文件获取、创建、更新、发布、upsert 后发布，以及配置文件/配置组 watch。
- 流量治理：`traffic` 域公开 router、ratelimit、circuitbreaker、faultdetect 和 policy 能力；旧顶层 `router`、`ratelimit`、`circuitbreaker`、`faultdetect` 路径仍保留兼容。
- 优雅上下线：`LosslessAPI` 支持 action provider 注册、延迟注册调度、实例级 register/deregister 和 readiness/offline endpoint。
- 外部 e2e：`e2e_tests` 可连接已启动的 control-plane console 和 client gRPC 端口，验证 connectivity、service-discovery、config-center 以及多类治理 case。

## 能力边界

README 的 quickstart 仍为空代码块；对外可用能力应优先以公开 API、public API 测试和 e2e README 为准。治理类 e2e 默认不会修改控制面，只有显式打开 `--execute-governance-control-plane` 时才执行 create、publish、SDK 行为断言和 cleanup。

## 证据

- `Cargo.toml`：crate 名称、版本、描述、workspace member、spec 依赖和发布排除。
- `README.md`：项目定位和安装说明。
- `src/lib.rs`：根公开模块。
- `src/discovery/api.rs`：ProviderAPI、ConsumerAPI、LosslessAPI。
- `src/config/api.rs`：ConfigFileAPI、ConfigGroupAPI。
- `src/traffic/mod.rs`：traffic 治理域聚合。
- `e2e_tests/README.md`：e2e 运行方式、环境变量、报告和覆盖范围。

## 相关页面

- [[sdk-module-architecture]]
- [[traffic-governance-architecture]]
- [[e2e-and-public-api-testing]]
