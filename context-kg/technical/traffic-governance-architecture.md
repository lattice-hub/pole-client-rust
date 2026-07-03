---
title: 流量治理架构
tags: [technical, traffic, governance]
links: [pole-rust-sdk-capabilities, sdk-module-architecture, e2e-and-public-api-testing]
updated: 2026-07-04
sources: 15
---

# 流量治理架构

流量治理能力统一收敛在 `traffic` 顶层域，包含 router、ratelimit、circuitbreaker、faultdetect 和 policy。旧顶层治理路径继续保留，用于兼容历史公开 API。

## 子域职责

- `traffic::router`：执行路由和负载均衡。`ProcessRouteRequest` 携带服务实例、路由信息和可选镜像请求，`ProcessRouteResponse` 返回过滤后的实例集合和 `TrafficGovernanceResult`。
- `traffic::ratelimit`：通过 `RateLimitAPI::get_quota` 获取配额，通过 `return_quota` 释放并发类配额；`QuotaRequest` 以 namespace、service、method 和流量标签提供器作为输入。
- `traffic::circuitbreaker`：通过 `CircuitBreakerAPI` 检查资源、上报统计，并生成 `InvokeHandler` 包装调用过程；资源粒度包括服务和方法。
- `traffic::faultdetect`：定义主动探测执行器和 reporter trait，探测计划支持 HTTP、TCP、UDP，结果可上报给熔断统计。
- `traffic::policy`：承载 traffic security、mirror 和 mock 的统一策略结果。`TrafficGovernanceResult` 包含安全判定、镜像目标和可选 mock response。
- `traffic::matcher`：内部共享 API 匹配候选索引。security、mirror、mock 和 ratelimit 可先按 method/path 收敛候选，再执行各自的 traffic match rule、参数匹配、采样、配额或结果构造。

## 规则和缓存映射

`discovery::req::ServiceRuleType` 将 Router、CircuitBreaker、RateLimit、Lane、FaultDetector、Lossless、TrafficSecurity、TrafficMirror、TrafficMock 映射到对应 `EventType`。`core::model::cache` 中的 `EventType`、`CacheItemType` 和 service rule cache item 承载远端规则落缓存后的类型边界。

## API 匹配索引

`traffic::matcher::ApiMatchIndex` 按 API method 分桶，并对 text exact path 建前缀树定位候选。前缀树只用于 exact path 的候选查找，不把 exact 改成前缀匹配语义；regex、非 text value、not-in/range 等复杂 `MatchString` 继续保留在 fallback 列表，并通过完整 `match_label_value` 判断。

security、mirror、mock 和 ratelimit 的规则遍历顺序仍由规则 priority 和子规则原始顺序决定。索引返回候选后，各能力继续执行原有能力特定判断：traffic security 执行动作和拒绝效果，mirror/mock 执行 traffic match rule 与百分比采样，ratelimit 执行参数匹配、failover 和本地计数器。

## 调度和副作用

`Engine::new` 会启动 client flow，并注册 fault-detect 资源监听；fault-detect listener 在规则和实例到达后构建探测 target。mirror 通过 `MirrorSender` 异步发送请求副本，security deny 和 mock response 通过 router 响应链路影响主请求结果。

## 证据

- `src/traffic/mod.rs`：traffic 顶层子域。
- `src/traffic/router/api.rs`：RouterAPI。
- `src/traffic/router/req.rs`：路由请求/响应和治理结果承载。
- `src/traffic/ratelimit/api.rs`：RateLimitAPI。
- `src/traffic/ratelimit/req.rs`：QuotaRequest/QuotaResponse。
- `src/traffic/circuitbreaker/api.rs`：CircuitBreakerAPI、InvokeHandler。
- `src/traffic/circuitbreaker/req.rs`：调用上下文和响应上下文。
- `src/traffic/faultdetect/api.rs`：探测执行和上报 trait。
- `src/traffic/faultdetect/req.rs`：探测计划、协议和结果模型。
- `src/traffic/policy/api.rs`：MirrorSender。
- `src/traffic/policy/req.rs`：security/mirror/mock 统一结果。
- `src/traffic/matcher.rs`：API method/path 候选索引和确定性候选收敛测试。
- `src/traffic/policy/default.rs`：security、mirror、mock 接入 API 候选索引后继续执行原有治理语义。
- `src/traffic/ratelimit/default.rs`：限流 trigger 接入 API 候选索引，并覆盖 `Api.path` 未命中不消耗本地配额。
- `src/discovery/req.rs`、`src/core/model/cache.rs`、`tests/public_api.rs`、`e2e_tests/src/control_plan.rs`：规则类型映射、公开路径和控制面端点覆盖。

## 相关页面

- [[pole-rust-sdk-capabilities]]
- [[sdk-module-architecture]]
- [[e2e-and-public-api-testing]]
