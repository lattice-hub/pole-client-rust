---
title: E2E 与公开 API 测试体系
tags: [quality, testing, e2e]
links: [pole-rust-sdk-capabilities, sdk-module-architecture, traffic-governance-architecture]
updated: 2026-06-17
sources: 8
---

# E2E 与公开 API 测试体系

项目测试体系由根 crate 的 public API 编译约束、单元测试和独立 `e2e_tests` workspace member 组成。`e2e_tests` 不发布，专门连接外部 control-plane console 和 client gRPC 端口验证 SDK 行为。

## Public API 约束

`tests/public_api.rs` 通过类型引用约束：

- `LosslessAPI` 构造函数仍公开。
- `traffic::faultdetect::{api,req}` 与 `traffic::policy::{api,req}` 暴露边界稳定。
- `traffic` 顶层域暴露 router、ratelimit、circuitbreaker、faultdetect、policy。
- 旧顶层 `router`、`ratelimit`、`circuitbreaker`、`faultdetect` 兼容路径仍可编译。

## E2E 覆盖

`e2e_tests` 默认 case 包括 connectivity、service-discovery、config-center、routing、ratelimit、circuitbreaker、lane-routing、fault-detect、lossless、mirror、auth-security、mock。

治理类 case 通过 `control_plan` 生成 create、publish 和 cleanup 动作。默认不直接修改控制面；只有开启 `--execute-governance-control-plane` 或对应环境变量时，才会执行控制面写操作和 SDK 行为断言。

## 报告和配置

运行配置支持命令行参数和环境变量，包括 console URL、client/discover/config 地址、token、case 选择、报告目录、跳过 cleanup、是否执行治理控制面。报告默认写到 `target/e2e-reports`，包括 JSON、JUnit XML 和 Markdown。

## 证据

- `tests/public_api.rs`：公开 API 路径和模块边界编译约束。
- `e2e_tests/Cargo.toml`：独立 workspace member、`publish = false` 和依赖。
- `e2e_tests/README.md`：运行方式、环境变量、报告和覆盖范围。
- `e2e_tests/src/cases.rs`：默认 case registry 和执行分支。
- `e2e_tests/src/control_plan.rs`：治理类控制面端点、create/publish/cleanup 计划。
- `e2e_tests/src/assertions.rs`：行为断言入口。
- `e2e_tests/tests/*`：e2e runner、控制面计划、流程、报告和 case 执行测试。
- `cargo metadata --format-version 1 --no-deps`：workspace target 和测试 target 列表。

## 相关页面

- [[pole-rust-sdk-capabilities]]
- [[sdk-module-architecture]]
- [[traffic-governance-architecture]]
