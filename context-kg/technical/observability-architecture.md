---
title: Rust SDK 观测能力架构
tags: [technical, observability, sdk]
links: [sdk-module-architecture, traffic-governance-architecture, e2e-and-public-api-testing]
updated: 2026-07-18
sources: 8
---

# Rust SDK 观测能力架构

Rust SDK 的观测能力以 `observability` 顶层模块承载，当前实现客户端语义基础：Resource attributes、OTel 环境变量、本地配置、OTLP endpoint 解析、结构化事件/指标模型和治理决策关联字段。真实 OpenTelemetry exporter 尚未接入，避免在指标低基数和治理链路语义稳定前引入不可验证的传输实现。

## 模块边界

- `observability::req`：定义 SDK 观测配置、Resource attributes、Telemetry endpoint、Signal 开关、MetricRecord、ObservabilityEvent、SpanAttributes 和 GovernanceDecisionContext。
- `observability::api`：定义 `ObservabilityRecorder` 扩展点和默认 no-op recorder。业务或后续 exporter 可以在该 trait 后面接 OpenTelemetry、日志或测试 recorder。
- `observability::default`：提供默认语义实现，包括从 `GlobalConfig`/环境变量生成观测状态，以及从直连配置生成 OTLP endpoint。
- `core::config::global`：`global.observability` 只承载 SDK 本地语义配置；连接地址位于 `global.serverConnectors.observability`。

## 配置来源

本地配置分为两部分：`global.observability` 支持 `enable`、`serviceName`、`serviceVersion`、`serviceInstanceId` 和 `events`；`global.serverConnectors.observability` 支持 `addresses`、`protocol`、`connectTimeout` 和 `messageTimeout`。SDK 直接向配置的 OTLP Collector 地址建立连接，不经服务发现二次寻址。

OTel 环境变量支持 `OTEL_SERVICE_NAME`、`OTEL_RESOURCE_ATTRIBUTES`、`OTEL_EXPORTER_OTLP_ENDPOINT`、`OTEL_TRACES_EXPORTER`、`OTEL_METRICS_EXPORTER` 和 `OTEL_LOGS_EXPORTER`。`OTEL_SERVICE_NAME` 和 `OTEL_RESOURCE_ATTRIBUTES` 会覆盖本地传入的同名 Resource attributes；`*_EXPORTER=none` 会关闭对应 signal。

## Endpoint 解析

`DefaultObservability::from_global_config` 优先使用 `OTEL_EXPORTER_OTLP_ENDPOINT`；未设置时按 `serverConnectors.observability.addresses` 生成 endpoint。`protocol` 包含 `http` 时使用 OTLP/HTTP，否则使用 OTLP/gRPC；`messageTimeout` 作为 exporter 超时。地址由部署配置承担高可用和变更，不在 SDK 内引入观测服务发现缓存或热切换状态机。

## 治理观测语义

`GovernanceDecisionContext` 为一次治理决策生成同一个 `decision_id`。metrics 属性只保留 namespace、主调服务、被调服务、规则类型和结果等低基数字段；event/span 属性允许携带 `pole.governance.decision.id` 和 `pole.governance.rule.id`，用于排障关联。

`MetricRecord::new` 和 `low_cardinality_metric_attributes` 会过滤 `rule.id`、`decision.id`、`trace_id`、`span_id`、`request_id`、`service.instance.id`、原始 URL path/client IP 等高基数字段，避免把事件级信息误放到指标标签。

## 证据

- `../pole-control-plane/context-kg/technical/adr/observability/adr-pole-rust-client-observability.md`：控制面 ADR 中定义 Rust SDK 观测职责、配置来源和验收约束。
- `src/observability/mod.rs`：观测顶层模块入口。
- `src/observability/api.rs`：`ObservabilityRecorder` 和 no-op recorder。
- `src/observability/req.rs`：Resource、endpoint、metric/event/span 和治理决策语义模型。
- `src/observability/default.rs`：env/config endpoint 解析默认实现。
- `src/core/config/global.rs`：`global.observability` 本地配置 schema 和解析测试。
- `tests/public_api.rs`：观测公共 API 编译边界。
- `src/traffic/*`：后续接入治理链路事件和指标时的能力来源。

## 相关页面

- [[sdk-module-architecture]]
- [[traffic-governance-architecture]]
- [[e2e-and-public-api-testing]]
