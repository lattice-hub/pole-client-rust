---
title: 知识库变更日志
tags: [meta, log]
links: [index]
updated: 2026-07-18
sources: 1
---

# 知识库变更日志

## [2026-07-18] ingest | 补齐规则驱动的分布式限流与连接配置收敛

- 新增页面：`technical/observability-architecture.md`
- 更新页面：`technical/sdk-module-architecture.md`、`tasks/todo.md`、`tasks/lessons.md`
- 变更摘要：发布公开 `polaris.metric.v2` Rust API 的 specification tag，SDK 按 `RateLimit.cluster` 懒加载并复用双向 gRPC 配额流；`serverConnectors` 收敛为 discover/config/observability 三个直连目标，观测 Collector 不再经 SDK 二次发现。

## [2026-07-17] ingest | 收敛 SDK 样例配置与 schema

- 新增页面：无
- 更新页面：`technical/sdk-module-architecture.md`、`_meta/log.md`、`tasks/todo.md`
- 变更摘要：将样例缓存配置迁移到 `consumer.localCache`，补齐 client identity 和默认负载均衡策略；删除失效 stat reporter 与治理模块本地开关，并以反序列化测试约束样例和 schema 同步。

## [2026-07-17] ingest | 撤销 SDK 动态治理开关

- 新增页面：无
- 更新页面：`technical/sdk-module-architecture.md`、`_meta/index.md`、`_meta/log.md`、`tasks/todo.md`
- 变更摘要：治理能力默认参与执行，实际效果只由控制面规则及规则自身状态决定；移除 SDK 模块级远端开关、订阅和 E2E case，避免两套状态来源。

## [2026-07-12] ingest | 记录 Rust SDK 观测能力基础架构

- 新增页面：`technical/observability-architecture.md`
- 更新页面：`_meta/index.md`、`_meta/log.md`、`tasks/todo.md`
- 变更摘要：根据控制面观测 ADR 和本仓实现记录 `observability` 顶层模块、`global.observability` 配置入口、OTel env 解析、Pole endpoint discovery、治理决策关联字段和 metrics 低基数保护边界。

## [2026-07-04] ingest | 记录流量治理 API 匹配索引

- 新增页面：无
- 更新页面：`technical/traffic-governance-architecture.md`、`tasks/todo.md`
- 变更摘要：记录 `traffic::matcher` 内部 API method/path 候选索引、exact path 前缀树语义边界，以及 security、mirror、mock、ratelimit 接入后的匹配职责划分。

## [2026-07-04] ingest | 整理治理前端设计文档元数据

- 新增页面：`fronted/design/*.md`
- 更新页面：`_meta/index.md`、`_meta/log.md`
- 变更摘要：为未跟踪治理前端设计交接稿补齐 context-kg frontmatter、相关页面区和索引入口，避免提交后留下已知 lint 失败状态。

## [2026-06-17] ingest | 从代码反向生成首批项目知识页

- 新增页面：`_meta/schema.md`、`business/pole-rust-sdk-capabilities.md`、`technical/sdk-module-architecture.md`、`technical/traffic-governance-architecture.md`、`quality/e2e-and-public-api-testing.md`
- 更新页面：`_meta/index.md`、`_meta/log.md`、`tasks/todo.md`
- 变更摘要：根据 Cargo metadata、README、公开 API、Engine、traffic 子域、配置加载、public API 测试和 e2e 工程反向生成项目能力、模块架构、流量治理和测试体系知识页。

## [2026-06-16] ingest | 扩展代码反向建库规则

- 新增页面：无
- 更新页面：`tasks/todo.md`、`tasks/lessons.md`
- 变更摘要：继续维护 `context-kg-maintainer` skill，补充从代码、测试、配置、公开 API 和构建元数据反向生成部分知识库的证据驱动流程。

## [2026-06-16] ingest | 更新 context-kg-maintainer skill 规则

- 新增页面：`_meta/index.md`、`_meta/log.md`
- 更新页面：`tasks/todo.md`、`tasks/lessons.md`
- 变更摘要：将当前仓库 `context-kg` 补齐到 lint 可识别的最小结构，并记录本次 skill 内置知识库分层和长期文档归档硬规则的维护任务。

## 相关页面

- [[index]]
