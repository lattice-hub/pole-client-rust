---
title: 切换 pole specification 依赖
tags: [task, review]
links: [lessons]
updated: 2026-07-04
sources: 1
---

# 切换 pole specification 依赖

## 本轮计划：治理匹配性能测试与 API path 索引设计

- [x] 读取现有 `traffic` 架构知识和任务经验，确认治理能力的模块归属
- [x] 检查 security、ratelimit、router、circuitbreaker、faultdetect、mirror、mock 的当前匹配入口
- [x] 收敛 API path 前缀树索引的适用范围、语义边界和测试口径
- [x] 补 RED 测试：限流 `Api.path` 不匹配不能消耗配额，API path 索引应把大量无关 exact path 规则收敛成少量候选
- [x] 新增 `traffic::matcher` 内部共享模块，支持 method 桶、exact path 前缀树、复杂 path fallback 和统一 API 语义
- [x] 将 traffic security、mirror、mock、ratelimit 的 API 判断改为共享 matcher，保留后续 traffic match rule / argument 的原有语义
- [x] 补齐 policy/ratelimit 行为等价测试和确定性候选收敛测试
- [x] 运行 fmt、warning-as-error check/test、diff check，并更新 review

### 设计说明

- 第一阶段只优化 `traffic` 域内已经具备 `Api` 入口的能力：traffic security、mirror、mock 和 ratelimit。
- exact path 使用前缀树做候选定位，但仍按 exact 语义命中；regex、not-in、range 等复杂 `MatchString` 保留 fallback，再走完整匹配，避免改变 spec 语义。
- router/lane 继续使用 `TrafficMatchRule` 和实例过滤链路；circuitbreaker/faultdetect 优先保持资源维度逻辑不变，后续如要优化 method resource key 单独处理。
- 性能测试不依赖 wall-clock 阈值，改用候选集数量和 predicate 调用次数验证匹配面收敛；耗时 benchmark 后续可作为非 CI 强约束补充。

## 本轮 review：治理匹配性能测试与 API path 索引设计

- 已完成：新增 `src/traffic/matcher.rs`，提供 `ApiMatchIndex`、`ApiMatchInput` 和统一 API method/path 匹配；exact text path 通过前缀树定位候选，regex 等复杂 path 保留 fallback 完整匹配。
- 已完成：traffic security、mirror、mock 已改为先按 API 索引收敛候选，再执行原有 `TrafficMatchRule`、采样和结果构造逻辑。
- 已完成：ratelimit trigger 已改为先按 API 索引收敛候选，再执行参数匹配、failover、本地 QPS/Concurrency 计数逻辑。
- 已完成：补充 RED 测试 `local_quota_counter_does_not_consume_quota_when_api_path_misses`，验证 `Api.path` 未命中不会错误消耗本地配额；该测试在修复前失败于第二次请求被限流。
- 已完成：补充 matcher 候选收敛测试，1000 条无关 exact path 规则不会进入 `/orders/target` 的候选结果，同时覆盖 fallback regex、空 API 列表全匹配和多 API 去重。
- 已完成：更新 `context-kg/technical/traffic-governance-architecture.md` 和 `_meta/log.md`，记录 matcher 的语义边界。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 107 个测试、public API 4 个测试、e2e_tests 64 个测试、doc tests 0 个全部通过。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`git diff --check` 通过。
- 已阻塞：`python3 /Users/chuntao.liao/.codex/skills/context-kg-maintainer/scripts/context_kg_lint.py ./context-kg` 已运行，但因本轮开始前已有的未跟踪 `context-kg/fronted/design/*.md` 缺少 frontmatter 失败；本轮未修改这些未跟踪文档。

## 本轮计划：更新 spec tag 到 v0.1.0-ALPHA.32

- [x] 将根包和 `e2e_tests` 的 `pole-specification` git tag 从 `v0.1.0-ALPHA.31` 更新到 `v0.1.0-ALPHA.32`
- [x] 运行 `RUSTFLAGS=-D warnings cargo check --workspace`，按最新 spec 的 breaking change 适配源码和测试
- [x] 运行 `cargo fmt`、旧品牌残留扫描、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 运行 `context_kg_lint.py ./context-kg` 和 `git diff --check`，记录既有未跟踪文档导致的 lint 阻塞
- [x] 更新本轮 review

## 本轮 review：更新 spec tag 到 v0.1.0-ALPHA.32

- 已完成：通过 `git ls-remote --tags --refs https://github.com/lattice-hub/specification.git` 确认最新 tag 为 `v0.1.0-ALPHA.32`。
- 已完成：根 `Cargo.toml` 和 `e2e_tests/Cargo.toml` 的 `pole-specification` git tag 已更新到 `v0.1.0-ALPHA.32`，Cargo 解析到提交 `70317c4d`。
- 已完成：适配 spec breaking change：治理规则里的单个 `api` 字段改为 `apis` 列表，SDK 侧按“空列表全匹配、任一 API 命中即可”处理 traffic security、mirror、mock。
- 已完成：适配限流 `LimitTrigger.method` 移除，改为从 `LimitTrigger.apis` 匹配请求 method；现有空 API 列表保持全匹配语义。
- 已验证：旧品牌残留扫描无输出。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。
- 已验证：`git diff --check` 通过。
- 已阻塞：`python3 /Users/chuntao.liao/.codex/skills/context-kg-maintainer/scripts/context_kg_lint.py ./context-kg` 已运行，但因本轮开始前已有的未跟踪 `context-kg/fronted/design/*.md` 缺少 frontmatter 失败；本轮未修改这些未跟踪文档。

## 本轮计划：更新 spec tag 到 v0.1.0-ALPHA.31

- [x] 将根包和 `e2e_tests` 的 `pole-specification` git tag 从 `v0.1.0-ALPHA.26` 更新到 `v0.1.0-ALPHA.31`
- [x] 更新本地依赖解析并运行 `RUSTFLAGS=-D warnings cargo check --workspace`
- [x] 按最新 spec 的 breaking change 适配源码和测试
- [x] 运行 `cargo fmt`、旧品牌残留扫描、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新本轮 review

## 本轮 review：更新 spec tag 到 v0.1.0-ALPHA.31

- 已完成：通过 `git ls-remote --tags --refs https://github.com/pole-io/specification.git` 确认最新 tag 为 `v0.1.0-ALPHA.31`。
- 已完成：根 `Cargo.toml` 和 `e2e_tests/Cargo.toml` 的 `pole-specification` git tag 已更新到 `v0.1.0-ALPHA.31`，Cargo 解析到提交 `7e596e46`。
- 已完成：适配 spec breaking change：故障探测规则从旧 wrapper 结构切换为 `FaultDetectRule` 列表，并从 `FaultDetectRule.rules` 展开子探测计划；内存缓存和 e2e downcast 同步更新。
- 已完成：适配熔断规则结构变化，`recover_condition` 和触发条件从规则顶层改为 `CircuitBreakerPolicy` / `block_config` 内读取，测试构造和 e2e 控制面 payload 同步更新。
- 已完成：适配 mirror/mock/security 规则结构变化，mirror/mock 子规则不再使用 `source` wrapper，改为规则级 `caller` 与子规则 `api` / `traffic_match_rule`；security 移除默认动作和拒绝状态码；mock response 改为业务 `code` / `body` 断言。
- 已完成：补齐 `source_match::Type::CallerService` 到 `ArgumentType::CallerService` 的映射。
- 已完成：修复 e2e case execution 测试 mock server 的 keep-alive 抖动，响应头加入 `connection: close`，并保留失败消息上下文。
- 已验证：旧品牌残留扫描无输出。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。

## 本轮计划：从代码反向更新项目知识库

- [x] 读取现有 `_meta/index.md`、`tasks/lessons.md` 和源码入口，确认当前知识库缺口
- [x] 从 `Cargo.toml`、`README.md`、`src/lib.rs`、主要模块、测试和 e2e 工程收集证据
- [x] 新增业务能力概览、技术模块架构、流量治理能力和测试体系知识页
- [x] 同步 `context-kg/_meta/index.md`、`context-kg/_meta/log.md` 和页面双向链接
- [x] 运行 `context_kg_lint.py` 和 `git diff --check`
- [x] 更新本轮 review

## 本轮 review：从代码反向更新项目知识库

- 已完成：新增 `context-kg/_meta/schema.md`，补齐本项目知识库的目录职责、frontmatter、链接和代码反向建库规则。
- 已完成：新增 `context-kg/business/pole-rust-sdk-capabilities.md`，从 Cargo metadata、README、公开 API 和 e2e README 归纳 SDK 对外能力边界。
- 已完成：新增 `context-kg/technical/sdk-module-architecture.md`，记录 `SDKContext`、`Engine`、配置加载、公开 API 分层、插件和依赖边界。
- 已完成：新增 `context-kg/technical/traffic-governance-architecture.md`，记录 traffic 顶层治理域、router/ratelimit/circuitbreaker/faultdetect/policy 子域、规则缓存映射和副作用边界。
- 已完成：新增 `context-kg/quality/e2e-and-public-api-testing.md`，记录 public API 编译约束、e2e case 矩阵、控制面执行开关、报告和配置。
- 已完成：更新 `context-kg/_meta/index.md`，新增 Meta、Business、Technical、Quality 分区；更新 `context-kg/_meta/log.md` 记录本次 ingest。
- 已验证：`python3 /Users/chuntao.liao/.codex/skills/context-kg-maintainer/scripts/context_kg_lint.py ./context-kg` 通过，9 个 Markdown 页面 frontmatter、链接和 index 基础检查通过。
- 已验证：`git diff --check -- context-kg` 通过。

## 本轮计划：扩展 context-kg-maintainer 支持代码反向建库

- [x] 在 `context-kg-maintainer/SKILL.md` 中新增从代码反向生成知识库的操作规程
- [x] 明确代码证据来源、可写入内容和禁止过度推断的边界
- [x] 将该能力纳入 ingest 工作流与触发描述
- [x] 将本次用户纠正沉淀到 `context-kg/tasks/lessons.md`
- [x] 更新 `context-kg/_meta/log.md`
- [x] 运行 skill 校验、context-kg lint、关键规则扫描和空白检查
- [x] 更新本轮 review

## 本轮 review：扩展 context-kg-maintainer 支持代码反向建库

- 已完成：`/Users/chuntao.liao/.codex/skills/context-kg-maintainer/SKILL.md` 的 frontmatter description 已加入 `reverse-generate structured knowledge`，便于后续在“从代码反向生成知识库”场景触发。
- 已完成：`Quick Start` 已要求从代码生成知识时先收集入口、模块边界、公开 API、测试、配置、迁移/spec 和依赖元数据等 source evidence。
- 已完成：新增 `Code Reverse Ingest` 操作，明确先读现有 `_meta/index.md` 和 lessons，再扫描代码面；输出聚焦模块职责、边界、数据流、扩展点和外部契约，不做逐文件流水账。
- 已完成：新增证据边界：测试和 examples 只能作为行为证据，不能直接推断产品意图；不确定意图必须作为 open question 或省略；页面 `sources` 要按实际使用的文件、manifest、测试、spec 或命令输出计数。
- 已完成：本次用户纠正已沉淀到 `context-kg/tasks/lessons.md`，并在 `context-kg/_meta/log.md` 追加记录。
- 已验证：`python3 /Users/chuntao.liao/.codex/skills/.system/skill-creator/scripts/quick_validate.py /Users/chuntao.liao/.codex/skills/context-kg-maintainer` 通过。
- 已验证：`python3 /Users/chuntao.liao/.codex/skills/context-kg-maintainer/scripts/context_kg_lint.py ./context-kg` 通过，4 个 Markdown 页面 frontmatter、链接和 index 基础检查通过。
- 已验证：`rg` 扫描确认 `reverse-generate`、`Code Reverse Ingest`、`source evidence`、`public APIs`、`technical/adr`、`## 证据` 等关键规则已写入 skill。
- 已验证：`git diff --check -- context-kg/tasks/todo.md context-kg/tasks/lessons.md context-kg/_meta/index.md context-kg/_meta/log.md` 通过。

## 本轮计划：更新 context-kg-maintainer skill 内置规则

- [x] 将知识库分层目录说明内置到 `context-kg-maintainer/SKILL.md`
- [x] 将长期文档归档硬规则内置到 skill 的工作流和放置规则中
- [x] 补充回答架构问题时优先读取 `_meta/index.md` 的查询规则
- [x] 将本次用户纠正沉淀到 `context-kg/tasks/lessons.md`
- [x] 运行 skill 校验与文本扫描验证
- [x] 更新本轮 review

## 本轮 review：更新 context-kg-maintainer skill 内置规则

- 已完成：`/Users/chuntao.liao/.codex/skills/context-kg-maintainer/SKILL.md` 新增 `Knowledge Base Layout`，内置 `_meta`、`business`、`technical`、`quality`、`tasks` 的默认目录职责。
- 已完成：skill 的 `Quick Start`、`Placement Rules`、`Hard Archiving Rules` 和 `Query` 已明确长期架构/技术方案归档到 `context-kg/technical/adr/`、业务知识进入 `business/`、质量知识进入 `quality/`、`tasks/` 只保留计划/进度/review/lessons。
- 已完成：架构和设计问题查询规则已明确从 `context-kg/_meta/index.md` 开始定位相关页面。
- 已完成：本次用户纠正已沉淀到 `context-kg/tasks/lessons.md`。
- 已完成：发现当前仓库 `context-kg` 缺少 `_meta/index.md` 且任务页无 frontmatter 后，补齐了最小 `_meta/index.md`、`_meta/log.md`，并为 `todo.md`、`lessons.md` 补充 frontmatter 与 `## 相关页面`。
- 已验证：`python3 /Users/chuntao.liao/.codex/skills/.system/skill-creator/scripts/quick_validate.py /Users/chuntao.liao/.codex/skills/context-kg-maintainer` 通过。
- 已验证：`python3 /Users/chuntao.liao/.codex/skills/context-kg-maintainer/scripts/context_kg_lint.py ./context-kg` 通过，4 个 Markdown 页面 frontmatter、链接和 index 基础检查通过。
- 已验证：`git diff --check -- context-kg/tasks/todo.md context-kg/tasks/lessons.md context-kg/_meta/index.md context-kg/_meta/log.md` 通过。
- 已验证：`rg` 扫描确认 `Knowledge Base Layout`、`Hard Archiving Rules`、`context-kg/_meta/index.md`、`context-kg/technical/adr`、`docs/design` 等关键规则已写入 skill。

## 本轮计划：全仓统一改名到 pole

- [x] 将本地 spec 依赖别名统一为 `pole-specification` / `pole_specification`
- [x] 将公开错误类型和内部引用统一为 `PoleError`
- [x] 将日志前缀、默认配置、测试数据、README、Cargo metadata 和示例统一到 `pole` / `Pole`
- [x] 扫描确认业务代码、文档、测试和配置中无旧品牌残留
- [x] 运行 `cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review

## 本轮 review：全仓统一改名到 pole

- 已完成：根包和 `e2e_tests` 的 spec 依赖别名统一为 `pole-specification`，代码导入统一为 `pole_specification::...`。
- 已完成：公开错误类型统一为 `PoleError`，examples、e2e、公开 API 测试和内部模块引用已同步。
- 已完成：日志前缀统一为 `[pole]`，默认配置文件名和环境变量切到 `pole.yaml` / `pole.yml` / `POLE_RUST_CONFIG`，测试数据中的默认命名空间、限流服务名和本地缓存目录也已同步。
- 已完成：README、Cargo metadata、LICENSE 文案和源码头注释已统一到 `Pole`；Cargo repository/homepage 与 README build badge 已指向 `pole-io/pole-client-rust`。
- 已验证：旧品牌残留扫描无输出。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。

## 本轮计划：主 crate 统一命名为 pole_rust

- [x] 将根 package/crate 名统一为 `pole_rust`
- [x] 更新 `e2e_tests` 对主 crate 的依赖名和所有外部引用
- [x] 更新 README、examples、tests、e2e 测试和显式客户端/示例服务名中的旧命名
- [x] 扫描确认旧主 crate 名无残留
- [x] 运行 `cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review

## 本轮 review：主 crate 统一命名为 pole_rust

- 已完成：根 `Cargo.toml` 的 package 名已改为 `pole_rust`，`cargo metadata` 解析到主 package 和 lib target 均为 `pole_rust`。
- 已完成：`e2e_tests/Cargo.toml` 中对主 SDK 的依赖名已改为 `pole_rust = { path = ".." }`。
- 已完成：`tests/public_api.rs`、examples、`e2e_tests` 里的外部 crate import 均已改为 `pole_rust::...`。
- 已完成：README 的 crate 名、docs.rs/crates.io badge 和安装示例已改为 `pole_rust`；显式客户端类型字符串和示例 provider service 名也已同步使用 `pole_rust`。
- 已验证：旧主 crate 名扫描无输出。
- 已验证：`cargo metadata --format-version 1 --no-deps` 解析到 `pole_rust` package、lib target 和 `e2e_tests` 对 `pole_rust` 的 path dependency。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。

## 本轮计划：更新 spec 最新 tag

- [x] 确认 `pole-io/specification` 官方远端最新 tag
- [x] 将根包和 `e2e_tests` 的 `pole-specification` git tag 更新到最新 tag
- [x] 更新 `Cargo.lock` 并检查是否有 spec breaking change
- [x] 运行 `cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review

## 本轮 review：更新 spec 最新 tag

- 已完成：通过 `git ls-remote --tags --refs https://github.com/pole-io/specification.git` 确认官方远端最新 tag 为 `v0.1.0-ALPHA.26`。
- 已完成：根 `Cargo.toml` 和 `e2e_tests/Cargo.toml` 的 `pole-specification` 依赖已从 `v0.1.0-ALPHA.25` 更新到 `v0.1.0-ALPHA.26`。
- 已完成：`Cargo.lock` 已锁定 `git+https://github.com/pole-io/specification.git?tag=v0.1.0-ALPHA.26#085f056a3b57abaf946bd5eae062f861ed7022b4`。
- 已完成：适配 `ALPHA.26` 的 spec breaking change：`TrafficSecurityRule`、`TrafficMirror`、`TrafficMock`、`RateLimit` 不再依赖规则顶层 `namespace/service` 字段；SDK 评估逻辑改为依赖 discovery/cache 的服务上下文，mirror/mock 继续按子规则 source 匹配服务；测试构造同步补齐 `MirrorSource.api`。
- 已验证：`cargo metadata --format-version 1 --no-deps` 解析到 `git+https://github.com/pole-io/specification.git?tag=v0.1.0-ALPHA.26`。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。

## 本轮计划：e2e_tests 提升为根 workspace member

- [x] 将 e2e 测试工具目录提升为根目录 `e2e_tests`
- [x] 更新 workspace member、发布 exclude、README 和任务记录中的路径
- [x] 删除空的 `crates` 残留目录
- [x] 运行 `cargo fmt`、`cargo test -p e2e_tests`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review

## 本轮 review：e2e_tests 提升为根 workspace member

- 已完成：e2e 测试工具现在位于根目录 `e2e_tests`，根 `Cargo.toml` 的 workspace member 已改为 `members = ["e2e_tests"]`。
- 已完成：`e2e_tests/Cargo.toml` 中主包依赖路径已从原两层路径修正为 `pole_rust = { path = ".." }`，package 仍为 `e2e_tests` 且保留 `publish = false`。
- 已完成：根包 `exclude` 已改为显式排除 `e2e_tests/*`；`cargo package --list --allow-dirty | rg '^e2e_tests/|^tests/|^examples/'` 无输出，确认发布清单不包含内部 e2e、根集成测试和 examples。
- 已完成：空的旧聚合目录已删除；旧聚合目录路径残留扫描无输出。
- 已验证：`cargo metadata --format-version 1 --no-deps` 解析到 `/Users/chuntao.liao/Github/pole-io/pole-client-rust/e2e_tests/Cargo.toml`。
- 已验证：`cargo fmt --all -- --check` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc cargo test -p e2e_tests` 通过，e2e 测试 64 个通过，doc tests 0 个。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。

## 本轮计划：e2e crate 内部测试命名调整

- [x] 将 e2e workspace member 目录改为 `e2e_tests`
- [x] 将 e2e package 名称改为 `e2e_tests`，保留 `publish = false`
- [x] 更新 README、测试引用、命令示例和 Cargo.lock
- [x] 在根包 `exclude` 显式加入 `e2e_tests/*`，降低发布误解
- [x] 运行 `cargo fmt`、`cargo test -p e2e_tests`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review

## 本轮 review：e2e crate 内部测试命名调整

- 已完成：e2e 测试工具已统一命名为 `e2e_tests`，workspace member 路径为 `e2e_tests`，package、lib target、bin target 和测试引用均使用该名称。
- 已完成：`e2e_tests/Cargo.toml` 继续保留 `publish = false`；根包 `Cargo.toml` 的 `exclude` 已显式加入 `e2e_tests/*`，发布包不会带入内部 e2e 工具。
- 已完成：`e2e_tests/README.md` 已更新命令示例，并移除早期“治理 case 只输出计划且会失败”的旧描述，改为当前 create/publish/SDK 行为断言/cleanup 语义。
- 已验证：`cargo metadata --format-version 1 --no-deps` 解析到 `e2e_tests` package、lib、bin 和测试 target。
- 已验证：`cargo package --list --allow-dirty | rg '^tests/|^examples/'` 无输出，确认根包发布清单不包含内部 e2e、根集成测试和 examples。
- 已验证：`cargo fmt --all` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc cargo test -p e2e_tests` 通过，e2e 测试 64 个通过，doc tests 0 个。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e 测试 64 个测试、doc tests 0 个全部通过。

## 本轮计划：traffic 顶层治理域迁移

- [x] 补 public API RED 测试，约束 `traffic::router`、`traffic::ratelimit`、`traffic::circuitbreaker`、`traffic::faultdetect`、`traffic::policy` 新路径可用
- [x] 将 `src/router` 物理迁移到 `src/traffic/router`，顶层 `src/router.rs` 保留 re-export shim
- [x] 将 `src/ratelimit` 物理迁移到 `src/traffic/ratelimit`，顶层 `src/ratelimit.rs` 保留 re-export shim
- [x] 将 `src/circuitbreaker` 物理迁移到 `src/traffic/circuitbreaker`，顶层 `src/circuitbreaker.rs` 保留 re-export shim
- [x] 将 `src/faultdetect` 物理迁移到 `src/traffic/faultdetect`，顶层 `src/faultdetect.rs` 保留 re-export shim
- [x] 将当前 `traffic` 内 security/mirror/mock 收进 `traffic::policy::{api,req,default}`，避免顶层 `traffic::api/req` 语义过宽
- [x] 更新内部引用优先使用 `crate::traffic::*` 新路径
- [x] 运行 `cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review

## 本轮 review：traffic 顶层治理域迁移

- 已完成：`router`、`ratelimit`、`circuitbreaker`、`faultdetect` 已物理迁移到 `src/traffic/{router,ratelimit,circuitbreaker,faultdetect}`，并继续保持各自 `api.rs`、`default.rs`、`req.rs`、`mod.rs` 组织。
- 已完成：`traffic` 原有 security/mirror/mock 治理策略相关类型已收敛到 `src/traffic/policy/{api,req,default}`，`traffic::policy` 作为该类策略能力的命名空间，避免继续占用过宽的 `traffic::api/req`。
- 已完成：顶层 `src/router.rs`、`src/ratelimit.rs`、`src/circuitbreaker.rs`、`src/faultdetect.rs` 只保留 re-export shim，兼容旧公开路径；新增 public API 测试同时约束新 `traffic::*` 路径和旧顶层兼容路径。
- 已完成：内部引用已优先改到 `crate::traffic::*` 新路径，`rg 'crate::(router|ratelimit|circuitbreaker|faultdetect)::|pole_rust::(router|ratelimit|circuitbreaker|faultdetect)::' src crates tests examples` 无输出。
- 已完成：残留目录检查通过，`find src -type d -empty -print` 无输出。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc cargo check --workspace` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc cargo test --test public_api -- --nocapture` 通过，4 个 public API 测试全部通过。
- 已验证：`cargo fmt` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 4 个测试、e2e crate 64 个测试、doc tests 0 个全部通过。

## 本轮计划：模块组织对齐与残留目录清理

- [x] 复核新增 `faultdetect`、`traffic` 模块是否和 `router`、`ratelimit`、`circuitbreaker` 的 `api/default/req` 组织一致
- [x] 补 RED 编译测试，约束公开入口走 `faultdetect::api`、`faultdetect::req`、`traffic::api`、`traffic::req`
- [x] 将 `faultdetect` 的 trait/API 与请求/结果模型从 `default.rs` 拆到 `api.rs`、`req.rs`，保留兼容 root re-export
- [x] 将 `traffic` 的镜像 sender trait 与请求/治理结果模型从 `policy.rs` 拆到 `api.rs`、`req.rs`，保留兼容 root re-export
- [x] 清理确认无引用的空残留目录
- [x] 运行 `cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新本轮 review

## 本轮 review：模块组织对齐与残留目录清理

- 已完成：`faultdetect` 已对齐为 `api.rs`、`default.rs`、`req.rs`，其中 `api.rs` 放 `FaultDetectProbeExecutor`/`FaultDetectReporter`，`req.rs` 放探测计划、配置、结果、target 类型，`default.rs` 保留调度器、listener、reporter 和协议执行实现。
- 已完成：`traffic` 已对齐为 `api.rs`、`policy.rs`、`req.rs`，其中 `api.rs` 放 `MirrorSender`，`req.rs` 放 `MirrorRequest`、`TrafficGovernanceRules`、`TrafficGovernanceResult`、`TrafficSecurityDecision`，`policy.rs` 保留治理评估和默认 mirror sender 实现。
- 已完成：保留兼容路径，`faultdetect` root 继续 re-export API/req/default，`traffic::policy` 继续 re-export 旧调用方使用的 sender/request/result 类型，同时新增 `traffic::api` 和 `traffic::req` 明确边界。
- 已完成：新增 `tests/public_api.rs::governance_modules_expose_api_and_req_boundaries`，先 RED 失败于缺少 `faultdetect::api/req` 和 `traffic::api/req`，拆分后 GREEN。
- 已完成：删除确认无引用且为空的 `src/spec/metric` 和 `src/spec` 残留目录；复查 `find src -type d -empty -print` 无输出。
- 已验证：`cargo fmt` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 2 个测试、e2e crate 64 个测试、doc tests 0 个全部通过。

- [x] 确认 crates.io 上 `pole-specification` 最新版本
- [x] 使用最小改动把 spec 依赖切到最新 `pole-specification`
- [x] 运行构建/测试验证依赖兼容性
- [x] 梳理新旧 spec API 的 breaking change 映射
- [x] 迁移受影响的模型、缓存、配置、路由和 gRPC 连接器代码
- [x] 再次运行构建/测试验证
- [x] 记录 review 结果

## 验证记录

- `cargo test`：失败。切换到 `pole-specification 0.1.0-ALPHA.22` 后出现 218 个编译错误，主要是生成 spec API 的 breaking changes，包括类型重命名、字段从 `Option<T>` 改为直接值、配置发现字段名变化，以及 tonic/prost 版本链变化。
- `cargo check`：通过。仍有既有 warning。
- `cargo test`：通过。9 个单元测试通过，doc test 0 个。

## Review

- `pole-specification` 最新版本确认为 `0.1.0-ALPHA.22`。
- 依赖已从旧 `pole-specification = "=1.5.4-2"` 切到 `pole-specification = "=0.1.0-ALPHA.22"`，并用 Cargo package rename 保留本地 `pole_specification` 引用路径。
- 同步升级 `prost/prost-build/prost-types/tonic` 到 0.14 系列，避免新 spec 生成 client 与本仓库 gRPC 类型版本不一致。
- 已迁移主要 breaking changes：`Option<T>` 字段改直接字段、`config_file` 改 `file`、`ConfigFileTag` 改 `labels`、`Routing` 改 `RouteRule/CustomRoute`、gRPC client 改 `DiscoverGrpcClient/ConfigGrpcClient`。
- 新 spec 没有 `upsert_and_publish_config_file` 同名 RPC，目前 connector 对该方法返回明确错误；后续如要恢复功能，需要按新服务端语义组合 create/update + publish 或确认另一个 spec 版本。
- 测试里的 tracing subscriber 初始化改为 `try_init()`，避免多测试模块重复设置全局 subscriber 导致 panic。

# 客户端组合 upsert 并发布配置

- [x] 补充 `ConfigPublishRequest` 拆分为文件写入请求和发布请求的测试
- [x] 在 gRPC connector 内部保留服务端 `Code`，用于 upsert 分支判断
- [x] 将 `upsert_publish_config_file` 改成 update/create + publish 组合调用
- [x] 运行定向测试、`cargo check` 和 `cargo test`
- [x] 记录 review 结果

## 验证记录

- `cargo test config_publish_request_splits_into_file_and_release_requests -- --nocapture`：通过，1 个定向测试通过。
- `cargo check`：通过。仍有既有 warning。
- `cargo test`：通过，10 个单元测试通过，doc test 0 个。

## Review

- `ConfigPublishRequest` 已支持拆分为 `ConfigFileRequest` 和 `ConfigReleaseRequest`，并补充单元测试覆盖字段映射。
- `ConfigPublishRequest::convert_spec()` 不再 `todo!()`，避免未来误用时运行时 panic。
- gRPC connector 新增内部 `send_create_config_file`、`send_update_config_file`、`send_publish_config_file`，保留服务端 `Code` 用于组合流程判断。
- `upsert_publish_config_file` 已改为客户端侧组合：先 `UpdateConfigFile`，若返回 `NotFoundResource` 则 `CreateConfigFile`，若并发创建导致 `ExistedResource` 则重试 `UpdateConfigFile`，最后调用 `PublishConfigFile`。
- 语义差异：新版 spec 的拆分 gRPC 接口没有服务端 `UpsertAndReleaseConfigFile` 的单事务边界；`md5` CAS 更新前检查只存在于服务端原子 upsert 方法中，客户端组合流程无法完全等价。

# 按 spec 补齐客户端治理能力

## 能力矩阵

- [x] spec 基线：当前 crates.io `pole-specification 0.1.0-ALPHA.22` 缺 `TrafficMock`、`TrafficSecurityRule`、`traffic_security_rules`、`traffic_mirror_rules`、`traffic_mock_rules` 等字段；服务端当前依赖的 `github.com/pole-io/specification v0.1.0-ALPHA.25` 已包含这些类型。当前 `Cargo.toml` 已切到官方 git tag `https://github.com/pole-io/specification.git#v0.1.0-ALPHA.25`，`Cargo.lock` 锁定提交 `78f368d9738137754b6f0c9489358eb9bb1c5f1e`。
- [x] 路由：已有 metadata / nearby / rule / lane 插件骨架；已补 `rule` 目标组匹配、目的组 isolate/priority 过滤和过滤实例回填；lane 已接入 `LaneGroup` 缓存并实现 strict/permissive 泳道过滤；nearby 已补健康实例比例降级判断和 strictNearby 不降级语义。
- [x] 限流：有 API 与插件骨架，`DefaultRateLimitAPI::get_quota` 已消除 `todo!()` 并接入 `RateLimit` 规则加载、本地 QPS 窗口计数、参数匹配和本地并发配额计数；已补 `RateLimitAPI::return_quota` 与本地并发配额释放；已支持 global 规则优先调用可注入 `DistributedQuotaClient`，远端失败时按 `FAILOVER_LOCAL` 降级成本地计数、按 `FAILOVER_PASS` 直接放行；已补 SDK 内部 `pole.metric.v2` 模块、真实 `RateLimitGRPCV2` gRPC quota client、`provider.rateLimit.addresses` 默认惰性装配，以及 INIT/ACQUIRE 双向流协议转换测试。
- [x] 熔断：有 API 与 composite 插件骨架，已修正 Open 状态应阻断请求的流转判断，补连续错误触发 Open 的规则状态机，并让 composite 插件的 `report_stat`/`check_resource` 能基于注入规则和资源 key 形成本地状态闭环；已补 `recoverCondition` 驱动的 Open -> HalfOpen -> Close 恢复窗口；已按配置启用 composite 插件、支持运行时更新远程规则，并在 `DefaultCircuitBreakerAPI` 的 `check_resource`/`report_stat` 前按资源 callee 加载 `CircuitBreakerRule` 后更新 flow；已为 `InvokeHandler` 私有路径补可注入规则 refresher，默认 API 创建 handler 时注入真实 refresher，在 acquire/report 前按 callee 刷新远程规则；Consumer 调用结果上报已接入本地熔断统计。
- [x] 泳道路由：缓存已有 `LaneGroup` 类型，已实现按 `TrafficMatchRule` 命中、lane label 过滤可用实例、strict 无实例返回空、permissive 无实例回退。
- [x] 探测：缓存已有 `FaultDetector` 类型，已补从 `FaultDetector` 生成 HTTP/TCP/UDP 探测计划的基础执行模型，并补 TCP 端口真实连接、TCP send/receive、HTTP method/url/header/body、UDP send/receive 探测执行；已修复 `MemoryCache` 写入 `FaultDetectRule` 时只 clone 临时值导致远程规则未落缓存的问题；已补 `FaultDetectScheduler`、`FaultDetectProbeExecutor`、`FaultDetectReporter`、默认探测执行器、周期调度入口和探测结果到熔断 `ResourceStat` 的上报映射；已补从 `ServiceInstances` 过滤可用实例并展开 host + plans 的目标构建，以及 scheduler 按 target 执行一次的入口；已补 `FaultDetectLifecycleOwner` 并由 `Engine` 持有，SDK 生命周期结束时可统一 abort 周期探测任务；已补 `FaultDetectResourceListener` 监听 `FaultDetectRule` 和 `Instance` 事件，规则与实例都到达后自动重建并启动探测 targets，`Engine` 创建后会注册这两个监听器。
- [x] 优雅上下线：API 骨架存在，`DefaultLosslessAPI` 已具备实例级 action provider 注册、register/deregister 调用基础能力；已补 `LosslessRule` 到 delay register、healthcheck、readiness、warmup/offline 执行计划的转换；已补 delay-by-time 异步延迟注册调度；已补 delay-by-healthcheck 轮询 `LosslessActionProvider::do_healthcheck()` 成功后再注册；已补 readiness/offline endpoint 状态模型、registry 生命周期接入和最小 HTTP 暴露入口。
- [x] 镜像：已基于 `TrafficMirror`/`MirrorRule` 实现命中 `TrafficMatchRule` 后返回镜像目标的执行入口，支持从缓存规则 downcast，并已接入 `RouterAPI` 响应链路返回 `MirrorDestination`；已补通用 `MirrorRequest`、`MirrorSender`、异步镜像请求副本调度入口和可配置 `HttpMirrorSender`；HTTP sender 已支持从 `ResourceCache::load_service_instances` 选取可用且标签匹配的目标实例生成 `http://ip:port`；已补 `ProcessRouteRequest.mirror_request`、`DefaultRouterAPI` 默认 HTTP mirror sender 装配和 route 命中镜像后的异步派发；已按 `mirror_percent` 做稳定采样；已补 `GrpcMirrorBody::uncompressed_unary` 和 `GrpcMirrorSender`，支持通过 HTTP/2 向 `grpc://` 静态目标或服务发现实例发送已 framing 的 unary gRPC body。
- [x] 鉴权/安全：已基于 `TrafficSecurityRule`/`TrafficSecurityPolicy` 实现 allow/deny 判定和拒绝效果输出，支持从缓存规则 downcast，并已接入 `RouterAPI` 直接把 deny 转成 `UNAUTHORIZED` 错误。
- [x] Mock：已基于 `TrafficMock`/`MockRule` 实现命中后返回 `MockResponse` 的执行入口，支持从缓存规则 downcast，并已接入 `RouterAPI` 在 mock 命中时短路后续实例路由。
- [x] 单元测试：已按 spec 资源映射、缓存/磁盘 failover、gRPC watcher key、ServiceRuleType、路由/lane/nearby、限流、熔断、探测、lossless、mirror/security/mock、Consumer/Provider API、Noop 插件和服务契约查询分层补齐；最终全量 `cargo test` 106 个单测通过，doc tests 0 个。

## 阶段计划

- [x] 阶段 1：把 Rust spec 依赖切到包含 mock/security/mirror 下发字段的 `v0.1.0-ALPHA.25` Git tag，并修复编译。当前已从本机 Go module cache path 改为官方 git tag，去掉本机绝对 path 依赖。
- [x] 阶段 2：扩展 `EventType`、`CacheItemType`、`ResourceEventKey`、内存缓存和磁盘 failover，覆盖 lossless、traffic security、traffic mirror、traffic mock。
- [x] 阶段 3：抽取统一 `TrafficMatchRule` 匹配器，覆盖 header/query/path/method 等参数来源；cookie、caller metadata、自定义标签已走统一入口但还需要更多场景测试。
- [x] 阶段 4：补齐 rule router 的目标组匹配、lane router、nearby router、lossless router / API 的执行语义。当前已完成 rule router 目标组匹配和 lane router；nearby 已补健康降级；lossless 已补 action provider 基础生命周期、`LosslessRule` 执行计划转换、delay-by-time 异步延迟注册调度、delay-by-healthcheck 探活成功后注册、readiness/offline endpoint 状态模型和最小 HTTP 暴露入口。
- [x] 阶段 5：实现限流本地配额、熔断状态机、故障探测执行器，并接入远程规则缓存。当前已补本地 QPS 窗口配额、参数匹配、并发配额与 `get_quota` 规则加载、`return_quota` 并发释放、global 限流远端 client 抽象和 `FAILOVER_LOCAL`/`FAILOVER_PASS` 降级语义，真实 `RateLimitGRPCV2` gRPC quota client 与 `provider.rateLimit.addresses` 默认惰性装配，熔断 Open/Close 流转修复、连续错误状态机、`recoverCondition` 半开恢复、composite 按资源 key 隔离的本地状态闭环、composite 插件启用与远程 `CircuitBreakerRule` 加载更新、`InvokeHandler` 私有路径规则刷新、`FaultDetectRule` 内存缓存写入修复、探测计划生成、HTTP/TCP/UDP 探测执行、探测周期调度入口、按可用实例展开探测 target、生命周期 owner 接入 Engine、结果上报到熔断统计，以及 `FaultDetectRule`/`Instance` 监听自动装载 targets。
- [x] 阶段 6：实现 traffic mirror、traffic security/auth、traffic mock 的客户端执行入口和结果模型。当前已补三类规则的执行入口、结果模型、缓存规则 downcast，并把缓存规则评估接入 `RouterAPI` 响应；安全 deny 已直接拦截，mock 已短路后续实例路由，镜像已具备通用异步请求副本调度入口、可配置 HTTP sender、基于服务发现实例缓存的 HTTP/gRPC 目标选址、RouterAPI 主链路可选 `MirrorRequest` 派发装配，以及 gRPC unary body framing 和 HTTP/2 发送能力。
- [x] 阶段 7：为每项能力补单元测试和必要的集成式内存缓存测试，最终 `cargo check`、`cargo test`、关键场景测试全部通过。

## 本轮 review

- 已完成：新增 lossless、traffic security、traffic mirror、traffic mock 的 discovery 请求映射、响应 watcher key、内存 cache item、磁盘 failover save/load、`ServiceRuleType` 上层枚举映射；新增统一 `TrafficMatchRule` 匹配器；补齐 rule router 目标组匹配、目的组 isolate/priority 过滤和过滤实例回填；补齐 lane router strict/permissive 过滤；补齐 lossless action provider 基础生命周期、`LosslessRule` 执行计划转换、delay-by-time 异步延迟注册调度、delay-by-healthcheck 探活成功后注册、readiness/offline endpoint 状态模型、registry 生命周期接入和最小 HTTP 暴露入口；补齐本地 QPS 窗口限流、参数匹配、并发配额基础能力、并发配额释放 API、global 限流可注入远端 client 抽象、failover 降级语义、SDK 内部 `pole.metric.v2` 类型、真实 `RateLimitGRPCV2` gRPC quota client 和 `provider.rateLimit.addresses` 默认惰性装配；补齐 traffic security/mirror/mock 执行入口、结果模型、缓存规则评估，并接入 `RouterAPI` 响应链路；安全 deny 已直接转 `UNAUTHORIZED`，mock 命中已短路后续实例路由；镜像已补通用异步请求副本调度入口、可配置 HTTP sender、基于服务发现实例缓存的 HTTP 目标选址、RouterAPI 主链路 mirror sender 装配、可选 `MirrorRequest` 异步派发和 `mirror_percent` 稳定采样；修复熔断 Open 放行判断、补连续错误状态机、`recoverCondition` 半开恢复，并让 composite 插件基于注入规则和资源 key 完成本地上报/检查闭环；已补 composite 插件配置启用、运行时规则更新、`CircuitBreakerRule` 服务规则 downcast、`DefaultCircuitBreakerAPI` check/report 前远程规则刷新，以及 `InvokeHandler` 可注入 refresher 和 acquire/report 前规则刷新；补齐 fault detect 探测计划生成、HTTP/TCP/UDP 探测执行、远程 `FaultDetector` 内存缓存写入修复、探测调度器、可用实例 target 展开、默认执行器、reporter 抽象、生命周期 owner 接入 Engine、熔断统计上报映射，以及 `FaultDetectResourceListener` 监听 `FaultDetectRule`/`Instance` 自动重建 target 并由 `Engine` 注册监听器；已把 ProviderAPI 的 `report_service_contract` 接到 Engine，修复上层服务契约上报路径的 `todo!()` panic；已实现 gRPC connector 的 `ReportClient` 上报，避免 SDK 后台 client flow 启动后因 `report_client` panic。
- 已验证：`cargo search pole-specification --limit 5` 确认 crates.io 最新仍是 `0.1.0-ALPHA.22`；`! rg 'path = "/' Cargo.toml` 通过，确认已移除本机绝对 path 依赖；`cargo metadata --format-version 1 --no-deps` 通过，解析到 `git+https://github.com/pole-io/specification.git?tag=v0.1.0-ALPHA.25`；直连 GitHub 的 `cargo check` 在本机出现 `SecureTransport error`、`Operation timed out`、`fetch-pack: unexpected disconnect` 后中断；使用临时 Git `insteadOf` 把同一个官方 URL 映射到本机 spec clone 后，`CARGO_NET_GIT_FETCH_WITH_CLI=true PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc cargo check` 通过，`pole-specification` 编译来源显示为 `https://github.com/pole-io/specification.git?tag=v0.1.0-ALPHA.25#78f368d9`；同样映射下 `CARGO_NET_GIT_FETCH_WITH_CLI=true PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc cargo test` 通过，90 个单测通过，doc tests 0 个。新增定向验证包括：`discovery::default::provider_tests::provider_report_service_contract_delegates_without_panicking` RED 先失败于 `todo!()`，实现后通过；`discovery::default::` 9 个单测通过。此前验证还包括：12 个限流单测、10 个熔断单测、15 个探测单测、11 个 traffic policy 单测、3 个 RouterAPI 治理单测、8 个 lossless 单测均已定向通过。
- 风险：crates.io 最新 `pole-specification` 仍是 `0.1.0-ALPHA.22`，尚未发布包含 mock/security/mirror 等字段的 `0.1.0-ALPHA.25` crate，所以当前依赖只能使用官方 git tag；本机直连 GitHub 拉取该 tag 曾超时，已用临时本地 URL 映射验证同一 tag/commit 的构建和测试；SDK 内部临时 vendored 了 `pole.metric.v2` 生成代码以接通 `RateLimitGRPCV2`，发布前应改为 spec crate 正式导出或可复现生成；spec 没有定义 gRPC mirror 专用字段，当前客户端约定 `MirrorRequest.body` 为 gRPC message body bytes（5 字节 gRPC message header + protobuf message bytes），不是 HTTP/2 raw frame。

## 本轮计划：lossless 延迟注册调度

- [x] 补 `LosslessExecutionPlan` delay-by-time 调度 RED 单测，确认注册动作不会同步触发
- [x] 实现最小 async 调度入口，按计划延迟后调用实例绑定的 `LosslessActionProvider::do_register`
- [x] 跑 lossless 定向测试、`cargo fmt`、`cargo check`、`cargo test`
- [x] 更新 review 和剩余风险

## 本轮计划：熔断远程规则加载

- [x] 补 composite 运行时远程规则更新 RED 单测，确认更新后新资源能按规则熔断
- [x] 补配置启用 composite 插件 RED 单测，确认 `consumer.circuitBreaker.enable` 会装载插件
- [x] 补 `CircuitBreakerRule` 服务规则 downcast 和资源 callee 提取测试
- [x] 实现 `CircuitBreakerFlow::update_rules` 与 `DefaultCircuitBreakerAPI` check/report 前远程规则刷新
- [x] 跑熔断定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：fault detect 缓存写入修复

- [x] 扩展 `MemoryCache::on_spec_event` 单测覆盖 `FaultDetectRule` 写入
- [x] 修复 `FaultDetectRule` 分支对临时 clone 写入导致缓存 value 未更新的问题
- [x] 跑 memory cache 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：fault detect 调度器和结果上报

- [x] 补 `FaultDetectScheduler::run_once` 与结果映射 RED 单测
- [x] 实现 `FaultDetectProbeExecutor`、`FaultDetectReporter`、默认执行器、熔断 reporter 和周期 spawn 入口
- [x] 跑 faultdetect 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：镜像实例选址和 lossless healthcheck 调度

- [x] 补 `HttpMirrorSender` 从服务实例缓存解析目标地址的 RED 单测
- [x] 实现静态 URL 优先、缓存实例兜底的 HTTP mirror 目标解析，并按 `MirrorDestination.labels` 过滤可用实例
- [x] 补 delay-by-healthcheck 失败后等待、成功后注册的 RED 单测
- [x] 实现 `LosslessActionProvider::do_healthcheck()` 轮询成功后再注册
- [x] 跑 traffic policy、lossless 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：fault detect 实例选择

- [x] 补 fault detect 可用实例展开 RED 单测
- [x] 实现从 `ServiceInstances` 生成 host + plan 探测目标
- [x] 补 scheduler 按 target 执行一次的 RED 单测并实现 `run_targets_once`
- [x] 跑 faultdetect 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：熔断 InvokeHandler 规则刷新

- [x] 补 `InvokeHandler` acquire/report 前刷新规则的 RED 单测
- [x] 实现可注入的熔断规则 refresher，并让默认 API 创建 handler 时注入真实 refresher
- [x] 跑熔断定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：lossless readiness/offline endpoint

- [x] 补 readiness/offline endpoint 状态机与 HTTP 处理 RED 单测
- [x] 实现最小 `LosslessEndpointState`，支持 readiness 上线后返回 ready、offline 触发下线后返回 offline
- [x] 将 endpoint 状态接入 `LosslessActionRegistry` 的 register/deregister 生命周期
- [x] 跑 lossless 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：镜像主链路 sender 装配

- [x] 补 RouterAPI mirror 主链路派发 RED 单测
- [x] 扩展 `ProcessRouteRequest` 携带可选 `MirrorRequest`
- [x] 让 `DefaultRouterAPI` 默认装配 `HttpMirrorSender` 并在 route 成功/命中 mock 时异步派发镜像请求
- [x] 跑 router/traffic 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：分布式限流客户端通道

- [x] 补 global 限流优先调用远端 quota client 的 RED 单测
- [x] 补远端失败时按 `FAILOVER_LOCAL` 和 `FAILOVER_PASS` 降级的 RED 单测
- [x] 抽象 `DistributedQuotaClient` 和请求/响应模型，并让 `DefaultRateLimitAPI` 可注入远端 client
- [x] 跑 ratelimit 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：探测生命周期 owner 接入

- [x] 补 `FaultDetectLifecycleOwner` 启停周期任务单测
- [x] 实现 owner 持有探测任务句柄并在 stop/drop 时 abort
- [x] 让 `Engine` 默认持有 owner，并暴露 target 启动入口
- [x] 跑 faultdetect 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：镜像百分比采样

- [x] 补 traffic mirror/mock 百分比采样边界单测
- [x] 实现基于 route 信息和规则 salt 的稳定采样，`0` 不命中、`>=100` 全命中
- [x] 跑 traffic policy 定向测试、`cargo fmt`、`cargo test`

## 本轮计划：探测规则监听自动装载

- [x] 补规则和实例事件到达后自动启动探测 target 的 RED 单测
- [x] 实现 `FaultDetectResourceListener`，监听 `FaultDetectRule`/`Instance` 并重建 targets
- [x] 让 `Engine` 创建后注册 fault detect 规则和实例监听器
- [x] 跑 faultdetect 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：限流 RateLimitGRPCV2 接线

- [x] 确认 crates.io 最新 `pole-specification` 与本地 spec source 的 metric v2 可用性
- [x] 补 SDK 能引用 metric v2 gRPC 类型的 RED 单测
- [x] 补真实 gRPC `DistributedQuotaClient` 请求/响应语义的 RED 单测
- [x] 实现 SDK 内部 metric v2 模块与真实 gRPC quota client
- [x] 跑 ratelimit 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：gRPC 镜像 sender

- [x] 补 gRPC unary message body framing helper 的 RED 单测
- [x] 补 `GrpcMirrorSender` 通过 HTTP/2 发送已 framing body 的 RED 单测
- [x] 实现 gRPC mirror 静态目标发送与最小 response 校验
- [x] 跑 traffic/router 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：spec 依赖可复现化

- [x] 确认当前 crates.io `pole-specification` 仍停在 `0.1.0-ALPHA.22`
- [x] 用 `! rg 'path = "/' Cargo.toml` 做 RED 检查，确认本机绝对 path 依赖存在
- [x] 将 `pole-specification` 切到官方 git tag `v0.1.0-ALPHA.25`
- [x] 跑 `cargo metadata`、`cargo check`、`cargo test` 验证依赖解析、编译和测试
- [x] 更新 review 和剩余风险

## 本轮计划：Provider 服务契约与 client 上报 panic 修复

- [x] 补 `DefaultProviderAPI::report_service_contract` 不应 panic 的 RED 单测
- [x] 实现 ProviderAPI 到 Engine 的服务契约上报委托
- [x] 实现 gRPC connector `ReportClient`，避免 SDK 后台 client flow panic
- [x] 跑 discovery 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：Consumer 调用结果上报接入熔断

- [x] 补 ConsumerAPI `report_service_call` 驱动连续失败熔断的 RED 单测
- [x] 扩展 `ServiceCallResult`，表达调用方、被调方、耗时、返回码、结果状态和可选方法维度
- [x] 实现 ConsumerAPI 到本地 `CircuitBreakerFlow::report_stat` 的统计转换
- [x] 跑 discovery/circuitbreaker 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：Consumer mock 命中短路负载均衡

- [x] 补 Consumer 层 mock 命中后返回 mock response、且不需要实例的 RED 单测
- [x] 扩展 `InstanceResponse` 承载可选 `MockResponse`
- [x] 让 `DefaultConsumerAPI::get_one_instance` 在 RouterAPI mock 命中时直接返回
- [x] 跑 discovery/router/traffic 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：weightedRoundRobin 首次调用与生命周期 no-op

- [x] 补 weightedRoundRobin `init/destroy` 不 panic 的 RED 单测
- [x] 补 weightedRoundRobin 首次选择实例不 panic 且按权重轮询的 RED 单测
- [x] 实现 cache 首次插入/过期清理和生命周期 no-op
- [x] 跑 loadbalance 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮增量 review：Consumer 上报、mock 短路与 weightedRoundRobin

- 已完成：`ConsumerAPI::report_service_call` 不再 `todo!()`，新增 `ServiceCallResult` 请求模型并转换为服务级/方法级 `ResourceStat`，直接进入本地 `CircuitBreakerFlow` 统计；`ConsumerAPI::get_one_instance` 在 RouterAPI 返回 mock 命中时直接返回 `InstanceResponse.mock_response`，不再继续负载均衡；`weightedRoundRobin` 的 `init/destroy` 改为 no-op，首次调用会初始化 cache，当前权重改为有符号原子以避免平滑轮询下溢。
- 已验证：`cargo fmt` 通过；`discovery::default::consumer_tests` 3 个通过；`circuitbreaker::` 10 个通过；`router::` 17 个通过；`traffic::` 11 个通过；`plugins::loadbalance::` 2 个通过；`cargo check` 通过；全量 `cargo test` 通过，95 个单测通过，doc tests 0 个。
- 后续增量已处理：`src/core/model/cache.rs` cache item、`src/core/plugin/cache.rs`/`src/core/plugin/connector.rs` Noop 实现、`src/plugins/connector/grpc/connector.rs::get_service_contract`、nearby 健康降级 TODO 均已去掉运行时 panic 或补齐语义。

## 本轮计划：cache item 基础状态语义去 panic

- [x] 补 cache item `is_loaded_from_file()` 和 `revision()` 不 panic 的 RED 单测
- [x] 实现当前内存 cache item 的 `is_loaded_from_file=false` 和 revision 返回
- [x] 跑 cache model 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：Noop cache/connector 去 panic

- [x] 补 NoopResourceCache 和 NoopConnector 生命周期/能力调用不 panic 的 RED 单测
- [x] 实现 Noop 插件生命周期 no-op、名称返回和能力调用明确错误
- [x] 跑 core plugin 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮计划：gRPC 服务契约查询和 nearby 健康降级

- [x] 补 `get_service_contract` 的 ServiceContracts discover 请求与契约筛选单测
- [x] 实现 gRPC connector 通过临时 Discover 流读取 `ServiceContracts` 并按契约身份筛选
- [x] 补 nearby 路由健康降级、健康可用不降级、strictNearby 不降级单测
- [x] 实现 nearby 基于选中层级不健康百分比的逐级降级，移除残留 TODO
- [x] 跑 connector/nearby 定向测试、`cargo fmt`、`cargo check`、`cargo test`

## 本轮增量 review：cache、Noop、服务契约查询与 nearby 降级

- 已完成：cache item 的 `is_loaded_from_file()` 统一返回当前内存态 `false`，`ServicesCacheItem::revision()` 返回真实 revision；`NoopResourceCache`/`NoopConnector` 生命周期改为 no-op，能力调用统一返回 `NotSupport`；`GrpcConnector::get_service_contract` 改为通过 `DiscoverRequestType::ServiceContracts` 查询并筛选契约；nearby 路由补齐健康降级语义，健康比例未达阈值时停在更近层级，达到阈值时向更宽层级降级，`strictNearby` 不降级。
- 已验证：`cargo test core::model::cache::cache_item_state_tests -- --nocapture` 通过，3 个测试；`cargo test noop_ -- --nocapture` 通过，2 个测试；`cargo test connector:: -- --nocapture` 通过，5 个测试；`cargo test plugins::router::nearby::nearby::tests -- --nocapture` 通过，3 个测试；最终 `cargo fmt` 通过，`cargo check` 通过，全量 `cargo test` 通过，106 个单测通过，doc tests 0 个。
- 剩余风险：当前仍有一批既有 warning（unused/dead_code/deprecated 字段等），未在本轮清理；直连 GitHub 拉取 spec tag 在本机此前不稳定，本轮验证继续使用同一官方 URL 到本机 spec clone 的临时 Git `insteadOf` 映射。

## 本轮计划：直接 spec tag 验证与功能复核

- [x] 确认 `Cargo.toml`/`Cargo.lock` 只使用官方 `specification.git` tag，不依赖本机 path 或 Git `insteadOf`
- [x] 不带本地 Git 映射运行 `cargo metadata`、`cargo check`、`cargo test`
- [x] 按功能矩阵复核路由、限流、熔断、泳道、探测、lossless、镜像、安全、mock、契约/上报链路和 Noop 兜底
- [x] 更新 review 结论

## 本轮增量 review：直接 tag 与公开能力复核

- 已完成：确认 `Cargo.toml` 使用 `pole-specification = { package = "pole-specification", git = "https://github.com/pole-io/specification.git", tag = "v0.1.0-ALPHA.25" }`，`Cargo.lock` 锁定 `git+https://github.com/pole-io/specification.git?tag=v0.1.0-ALPHA.25#78f368d9738137754b6f0c9489358eb9bb1c5f1e`；确认本仓库和全局 Git 配置没有 `url.*.insteadOf` 重写；`cargo metadata` 直接解析到官方 git tag。复核功能矩阵时发现 lossless 已实现但 API 仍是 `pub(crate)`，已将 `new_lossless_api`、`new_lossless_api_by_context`、`LosslessAPI` 改为公开，并新增 `tests/public_api.rs` 从外部 crate 视角验证公开入口可导入。
- 已验证：不带任何本地 Git 映射，仅设置 `PROTOC` 后，`cargo metadata --format-version 1 --no-deps` 通过，`cargo check` 通过，`cargo test` 通过。当前测试结果为库内 106 个单测通过，`tests/public_api.rs` 1 个集成测试通过，doc tests 0 个。`rg 'todo!\(|unimplemented!\(|TODO|todo' src tests` 无结果。
- 剩余风险：仍有既有 warning（unused/dead_code/deprecated 字段等），但不影响 check/test；`pole.metric.v2` 仍是 SDK 内部临时生成模块，后续最好等 spec crate 正式导出后替换。

## 本轮计划：移除本地 metric v2 并重拆模块

- [x] 确认官方 spec tag 的公开 Rust API 不导出 `pole.metric.v2`，记录限流能力边界
- [x] 补充限流 global 规则在无 metric v2 远端协议时按 failover 降级的测试
- [x] 移除 SDK 内部 `src/spec/metric/v2.rs` 与相关 gRPC client 装配，保留可注入 `DistributedQuotaClient` 抽象
- [x] 按现有 SDK 风格拆分 `ratelimit/default.rs` 与 `faultdetect/mod.rs`，让 `mod.rs` 只承担模块声明和公开 re-export
- [x] 运行静态检查确认不再引用 `crate::spec` / `pole.metric.v2` / `RateLimitGRPCV2`
- [x] 运行 `cargo fmt`、`cargo check`、`cargo test` 并补充 review

## 本轮增量 review：移除 metric v2 与模块拆分

- 已完成：确认官方 spec tag `v0.1.0-ALPHA.25` 的 Rust crate `lib.rs` 只公开 `pub mod v1;`，没有公开导出 `pole.metric.v2`；已删除 SDK 内部 `src/spec/metric/v2.rs`、`src/spec/metric/mod.rs`、`src/spec/mod.rs`，并从 `src/lib.rs` 移除 `pub(crate) mod spec;`。限流不再默认从 `provider.rateLimit.addresses` 装配 `RateLimitGRPCV2` client，保留可注入 `DistributedQuotaClient` 抽象；global 限流在无远端 client 或远端失败时按规则 `FAILOVER_LOCAL` / `FAILOVER_PASS` 降级。
- 已完成：新增 `global_quota_without_distributed_client_falls_back_to_local_when_configured`、`global_quota_without_distributed_client_passes_when_configured`、`distributed_quota_request_carries_rule_trigger_and_amounts` 单测；`ratelimit` 的分布式配额模型拆到 `src/ratelimit/distributed.rs`；`faultdetect/mod.rs` 改为只声明 `mod default; pub use default::*;`，实现移动到 `src/faultdetect/default.rs`，保持原有 `crate::faultdetect::*` 调用路径不变。
- 已验证：`rg 'crate::spec|pole\\.metric\\.v2|RateLimitGRPC|RateLimitGrpc|metric_v2|GrpcDistributedQuotaClient|rate_limit_grpc_endpoint' src tests Cargo.toml` 无结果；`cargo fmt` 通过；`cargo metadata --format-version 1 --no-deps` 解析到 `git+https://github.com/pole-io/specification.git?tag=v0.1.0-ALPHA.25`；`cargo test ratelimit::default::tests -- --nocapture` 12 个通过；`cargo test faultdetect::default::tests -- --nocapture` 15 个通过；`cargo check` 通过；全量 `cargo test` 通过，库内 106 个单测、`tests/public_api.rs` 1 个集成测试、doc tests 0 个全部通过。
- 剩余风险：当前 spec tag 源码目录里仍包含未公开的 `pole.metric.v2.rs` 生成文件，但 crate 公共 API 不导出；因此 SDK 不能可靠地内建真实 `RateLimitGRPCV2` client。若后续 spec 正式公开该模块，可以在不恢复本地 `src/spec` 的前提下实现官方 spec client 适配。既有 warning 仍存在，本轮未扩大范围清理。

## 本轮计划：清理 warning 和历史包袱

- [x] 将 `cargo check` / `cargo test` 的 warning 列表作为待修复清单，不再把 warning 归为剩余风险
- [x] 清理无用 import、无用变量和不必要的 mut
- [x] 删除或收窄未接入的私有字段、函数、测试辅助代码，避免生产编译残留历史包袱
- [x] 处理 deprecated spec 字段用法，改成当前 spec 推荐字段
- [x] 用 `RUSTFLAGS=-D warnings` 跑 `cargo check` 和 `cargo test`，确保 warning 归零
- [x] 更新 review 结论

## 本轮增量 review：warning 归零

- 已完成：清理无用 import、无用变量、不必要 `mut`、未使用常量和仅测试使用的 RSA helper；`Engine` 中只用于持有后台任务生命周期的字段改为 `_client_flow`，移除未读 `client_ctx` 字段；删除空壳 `RatelimitFlow`。`InvokeHandler::acquire_permission`、`on_success`、`on_error` 改为公开方法，避免 API 返回 handler 但外部无法调用；同时修复 `on_error` 错误调用 `ResultToErrorCode::on_success` 的问题。
- 已完成：移除无真实生产入口的限流 `DistributedQuotaClient` / `DistributedQuotaRequest` 抽象和相关测试，不再保留“看起来有远端限流 client、实际没有官方 spec 协议支撑”的历史包袱；global 限流保留当前可证明的 `FAILOVER_LOCAL` / `FAILOVER_PASS` 语义。lossless 已把延迟注册和 readiness/offline endpoint server 接到公开 `LosslessAPI::schedule_register` / `serve_endpoint`。
- 已完成：熔断规则测试和 composite 状态机改用新 spec 的 `block_configs.trigger_conditions`，不再读写 deprecated `trigger_condition` 字段；测试模块改为显式导入测试依赖类型，生产模块不再为测试保留 import。
- 已验证：`cargo fmt` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test` 通过，库内 102 个单测、`tests/public_api.rs` 1 个集成测试、doc tests 0 个全部通过；`rg 'todo!\(|unimplemented!\(|TODO|todo|crate::spec|pole\.metric\.v2|RateLimitGRPC|RateLimitGrpc|metric_v2|GrpcDistributedQuotaClient|RatelimitFlow|DistributedQuota' src tests Cargo.toml` 无结果。

## 本轮计划：设计独立 e2e crate

- [x] 审计当前仓库是否已有可接入 pole-control-plane 的 e2e 测试体系
- [x] 明确 e2e crate 的输入参数、覆盖范围、资源准备/清理和失败诊断方式
- [x] 给出 2-3 种实现方案并确认设计
- [ ] 用户确认后再写设计文档和实施计划

## e2e 体系设计草案

### 现状

- 当前仓库没有完整 e2e 测试体系；已有覆盖主要是库内单元测试和 `tests/public_api.rs` 公开 API 编译级集成测试。
- SDK 已可通过 `SDKContext::create_by_addresses(vec![...])` 接入外部 control-plane，地址形态应支持 `discover://host:port` 和 `config://host:port`。
- `pole-control-plane` 的 console 命名、配置、鉴权、命名空间入口分别通过 `/naming/v1/*`、`/config/v1/*`、`/auth/v1/*`、`CoreURL/*` 反向代理到后端 server，适合作为 e2e 的资源准备/清理入口。

### 推荐方案：独立 workspace crate

- 新增 `e2e_tests`，作为独立测试/报告 crate，不污染主 SDK 包发布面。
- 输入参数：
  - `--console-url` / `POLE_E2E_CONSOLE_URL`：例如 `http://127.0.0.1:8080`。
  - `--client-addr` / `POLE_E2E_CLIENT_ADDR`：同时用于 discover/config，例如 `127.0.0.1:8091`。
  - `--discover-addr`、`--config-addr`：覆盖拆分端口场景。
  - `--user`、`--password`、`--token`：用于 console/auth 受保护接口。
  - `--case`、`--skip-cleanup`、`--report-dir`：控制执行范围、保留现场和报告目录。
- 执行模型：
  - 通过 console HTTP API 创建 namespace/service/rule/config/auth policy 等控制面资源。
  - 通过 pole-client-rust 公开 API 连接 client gRPC 端口，验证资源下发和客户端行为。
  - 每个 case 使用唯一前缀 `e2e-<timestamp>-<case>`，执行结束反向清理；失败时支持保留资源并输出诊断。
  - 输出 JUnit XML、JSON 明细和 Markdown 摘要到 `target/e2e-reports/`。

### 覆盖矩阵

- 基础连通：console 健康检查、server functions/bootstrap、SDK discover/config 连接。
- 服务发现：namespace/service 创建、provider register/heartbeat/deregister、consumer get_one/get_all/watch。
- 配置中心：create/update/publish/upsert_publish/watch。
- 路由：metadata/rule 路由、目标组优先级/isolate、nearby 降级。
- 限流：本地限流规则下发、QPS/并发配额、failover local/pass。
- 熔断：规则下发、连续错误打开、半开恢复、Consumer report_service_call 闭环。
- 泳道路由：lane label 命中、strict/permissive 回退。
- 探测：HTTP/TCP/UDP 探测 target 下发、失败结果驱动熔断统计。
- 优雅上下线：delay register、healthcheck register、readiness/offline endpoint。
- 镜像：HTTP/gRPC mirror 目标接收副本、主链路不阻塞。
- 鉴权/安全：deny 规则返回未授权、allow 规则放行。
- Mock：mock 命中后 consumer 返回 mock response 且不依赖实例。

### 备选方案

- 轻量方案：只在 `tests/e2e` 放 ignored integration tests，通过环境变量传参。实现快，但报告、清理、case 编排和诊断能力弱。
- 重型方案：用 docker compose 或 testcontainers 自动拉起 control-plane。可复现性最好，但和“输入 console/client 端口验证已有环境”的目标不完全一致，依赖更重。

### 待确认

- 推荐采用独立 `e2e_tests`，先实现基础连通、服务发现、配置、mock/安全/路由 5 条 P0 流程，再扩展限流、熔断、泳道、探测、lossless、镜像。
- 需要确认 control-plane 默认账号/鉴权策略；若环境关闭鉴权，e2e harness 应允许 `--token` 为空并跳过 login。

## 本轮计划：落地 e2e crate 骨架

- [x] 将根 `Cargo.toml` 改成 package + workspace 共存，并新增 `e2e_tests`
- [x] 先写 RED 测试覆盖 CLI/env 参数解析、client 地址规范化、报告序列化、case 注册
- [x] 实现 `e2e_tests` 的配置、报告、runner 和 case registry
- [x] 提供 `e2e_tests` 二进制入口，支持 `--console-url`、`--client-addr`、`--discover-addr`、`--config-addr`、`--token`、`--case`、`--report-dir`、`--skip-cleanup`
- [x] 首批 case 覆盖 `connectivity`、`service-discovery`、`config-center`；治理类 case 已注册但未实现时明确失败，避免假通过
- [x] 输出 JSON、JUnit XML、Markdown 三类报告，默认目录为 `target/e2e-reports`
- [x] 跑 `cargo test -p e2e_tests`、`cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`

## 本轮 e2e review

- 已完成：新增独立 workspace member `e2e_tests`，提供二进制入口和库内 harness；支持从 CLI/env 输入 console URL、共享 client 地址或拆分 discover/config 地址、token、case filter、报告目录和 skip cleanup。
- 已完成：新增 console connectivity probe，访问 `/admin/v1/server/functions`；新增真实 SDK case `service-discovery` 和 `config-center`，分别覆盖 provider register/heartbeat/consumer discover/deregister，以及 config upsert_publish/get。
- 已完成：注册完整能力矩阵 case：`connectivity`、`service-discovery`、`config-center`、`routing`、`ratelimit`、`circuitbreaker`、`lane-routing`、`fault-detect`、`lossless`、`mirror`、`auth-security`、`mock`。未实现的治理类 case 会明确失败，不会被 skipped 掩盖为通过。
- 已完成：输出 JSON、JUnit XML、Markdown 三类报告，默认目录 `target/e2e-reports`；新增 README 记录运行方式、环境变量和当前覆盖边界。
- 已验证：`cargo test -p e2e_tests` 通过，新增 e2e crate 12 个测试通过；`cargo fmt` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过；同样环境下 `RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 12 个测试全部通过。
- 剩余工作：治理类 case 目前只注册并 fail-fast，还需要继续接入 control-plane 规则创建/清理和 SDK 行为验证，覆盖路由、限流、熔断、泳道、探测、lossless、镜像、鉴权/安全、mock 的端到端流程。

## 本轮计划：e2e 控制面编排基础

- [x] 增加 `ConsoleClient`，支持 console base URL、Bearer token、JSON GET/POST、HTTP 非 2xx 错误诊断
- [x] 增加 `CleanupPlan` / `ControlPlaneAction`，按 LIFO 顺序表达资源清理动作
- [x] 从 `pole-control-plane` HTTP server 源码确认治理端点，并固化治理 case 的 create/publish/delete endpoint 映射
- [x] 让治理类 case 输出包含 create/publish/cleanup 端点的失败诊断，不再只是泛化未实现
- [x] 跑 `cargo fmt`、`cargo test -p e2e_tests`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`

## 本轮 e2e 控制面编排 review

- 已完成：新增 `e2e_tests/src/console.rs`，统一 console HTTP JSON 访问，支持 Bearer token、GET/POST、非 2xx 错误消息和空 body 处理。
- 已完成：新增 `CleanupPlan` 和 `ControlPlaneAction`，用 LIFO 顺序表达资源清理，后续 case 可在创建实例、服务、规则发布版本后反向清理。
- 已完成：新增 `control_plan`，根据 `pole-control-plane/plugin/apiserver/httpserver/discover/*.go` 中的路由注册固化治理 case 的 create/publish/release delete/rule delete 端点，包括 routings、ratelimits、circuitbreakers、lane groups、faultdetectors、lossless、traffic mirrors/security/mocks。
- 已完成：治理 case 现在失败消息会带出控制面计划端点，便于报告中定位下一步缺失；仍然保持失败而非 skipped，避免假通过。
- 已验证：`cargo test -p e2e_tests` 通过，e2e crate 当前 18 个测试通过；`cargo fmt` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过；同样环境下 `RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 18 个测试全部通过。
- 剩余工作：需要继续把 `control_plan` 的占位 JSON 替换为 spec 对应的真实规则 payload，并把 `ConsoleClient` 真实执行 create/publish/cleanup 接入每个治理 case，然后补 SDK 行为断言。

## 本轮计划：控制面计划执行器

- [x] 增加控制面计划执行器，按 create -> publish -> cleanup 顺序执行，并在 publish 失败时仍然清理
- [x] 增加 `--execute-governance-control-plane` / `POLE_E2E_EXECUTE_GOVERNANCE_CONTROL_PLANE` 开关，默认关闭，避免占位 payload 误操作环境
- [x] 治理类 case 在开关打开时真实调用 console create/publish/cleanup，并将步骤结果写入失败诊断；行为断言补齐前仍不允许通过
- [x] 跑 `cargo fmt`、`cargo test -p e2e_tests`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`

## 本轮控制面计划执行器 review

- 已完成：`ControlPlaneAction` 可由 `ConsoleClient::execute_action` 执行，`execute_control_plane_plan` 会按 create -> publish -> cleanup 顺序执行，并在 publish 失败时继续执行所有 cleanup actions。
- 已完成：新增 `ControlPlaneExecution`、`ControlPlaneStepReport`、`ControlPlaneExecutionStatus`，报告每一步路径、成功状态和错误消息；治理 case 打开执行开关后会把这些结果汇总进 case message。
- 已完成：新增 `--execute-governance-control-plane` / `POLE_E2E_EXECUTE_GOVERNANCE_CONTROL_PLANE`，默认关闭；打开后治理 case 会真实调用 console 端点执行 create/publish/cleanup，但在 SDK 行为断言未补齐前仍然失败，不会误报通过。
- 已验证：`cargo test -p e2e_tests` 通过，e2e crate 当前 20 个测试通过；`cargo fmt` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过；同样环境下 `RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 20 个测试全部通过。
- 剩余工作：继续替换真实治理规则 payload，并逐个 case 实现 SDK 行为断言；当前开关主要用于验证控制面生命周期和清理语义。

## 本轮计划：mock/security 真实规则 payload

- [x] 从 `specification/source/rust/pole-specification/proto/mock.proto`、`traffic_security.proto`、`router.proto` 确认 TrafficMock、TrafficSecurityRule 和 TrafficMatchRule JSON 字段
- [x] 补 RED 测试约束 mock payload 包含 `rules.source.traffic_match_rule`、`response`、`mock_percent`
- [x] 补 RED 测试约束 security payload 包含 `policies.traffic_match_rule`、`action=TRAFFIC_SECURITY_DENY`、`reject_effect` 和 `default_action=TRAFFIC_SECURITY_ALLOW`
- [x] 将 `control_plan` 中 mock/security 的 create body 从通用占位 JSON 替换为 spec-like 规则 payload
- [x] 跑 `cargo fmt`、`cargo test -p e2e_tests`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`

## 本轮 mock/security payload review

- 已完成：`mock` control-plane create body 已从通用占位结构替换为 spec-like `TrafficMock` JSON，包含 `rules.source.traffic_match_rule`、`response`、`mock_percent=100`、`metadata`。
- 已完成：`auth-security` control-plane create body 已从通用占位结构替换为 spec-like `TrafficSecurityRule` JSON，包含 `policies.traffic_match_rule`、`action=TRAFFIC_SECURITY_DENY`、`reject_effect.status_code=403`、`default_action=TRAFFIC_SECURITY_ALLOW`、`metadata`。
- 已完成：README 标注 mock/security 已具备 spec-like payload，其他治理类型仍需逐步替换真实 payload。
- 已验证：`cargo test -p e2e_tests` 通过，e2e crate 当前 22 个测试通过；`cargo fmt` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过；同样环境下 `RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 22 个测试全部通过。
- 剩余工作：mock/security 还需要接 SDK 行为断言，验证规则下发后 mock 短路和 security deny 的实际客户端结果；其他治理 case 仍需真实 payload。

## 本轮计划：mock/security SDK 行为断言输入

- [x] 补 RED 测试，约束 mock/security 控制面 payload 使用可由客户端静态 `traffic_label_provider` 复现的标签值
- [x] 补 RED 测试，约束 mock/security e2e flow 构造 `GetOneInstanceRequest` 并携带匹配控制面规则的 header 标签
- [x] 实现 mock/security 专用 e2e flow 输入和稳定标签常量
- [x] 补 RED 测试，要求控制面 setup 后先执行 mock SDK 行为断言，再 cleanup
- [x] 拆分控制面 setup/cleanup 生命周期，并把 mock/security SDK 行为断言接入 live e2e 路径
- [x] 跑 `cargo test -p e2e_tests`、`cargo fmt`、`RUSTFLAGS=-D warnings cargo check --workspace`、`RUSTFLAGS=-D warnings cargo test --workspace`
- [x] 更新 review 和剩余风险

## 本轮计划：剩余治理 payload spec-like 化

- [x] 补 RED 测试约束 routing、ratelimit、circuitbreaker、lane、fault-detect、lossless、mirror 的关键 payload 字段
- [x] 将上述治理 case 的 create body 从通用占位 JSON 替换为 spec-like 最小规则结构
- [x] 保持所有治理 payload 都带 e2e metadata，便于诊断和清理
- [x] 跑 e2e crate 全量和 workspace 严格验证
- [x] 更新 review 和剩余风险

## 本轮 e2e 增量 review：治理 payload 与 mock/security 行为断言

- 已完成：新增 `assertions` 模块，明确 mock 命中和 security deny 的 SDK 结果判定；mock 需要返回 `InstanceResponse.mock_response` 且 `status_code=200`、`code=E2E_MOCK`、body 包含 `mocked=true`，security 需要返回 `UNAUTHORIZED` 或 `traffic security denied`。
- 已完成：mock/security e2e flow 使用稳定 header 标签构造 `GetOneInstanceRequest.route_info.traffic_label_provider`，并让控制面 payload 复用同一语义值；避免使用动态 run_id 导致 `fn` 指针无法捕获上下文。
- 已完成：控制面执行拆成 setup 和 cleanup，mock/security live e2e 会在 create/publish 之后、cleanup 之前调用真实 SDK `ConsumerAPI::get_one_instance` 做行为断言；cleanup 仍会执行，避免失败后残留规则。
- 已完成：ratelimit live e2e 已接入真实 `RateLimitAPI::get_quota` 行为断言，使用 `x-pole-e2e-ratelimit=limited` 匹配本地 QPS=1 规则，要求第一次配额通过、第二次配额拒绝。
- 已完成：routing、ratelimit、circuitbreaker、lane-routing、fault-detect、lossless、mirror 的 create body 已从通用占位 JSON 替换为 spec-like 最小规则结构，覆盖关键字段和 e2e metadata。
- 已验证：`cargo test -p e2e_tests -- --nocapture` 通过，e2e crate 41 个测试通过；`cargo fmt` 通过；`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过；同样环境下 `RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 41 个测试、doc tests 0 个全部通过。
- 剩余风险：routing、circuitbreaker、lane-routing、fault-detect、lossless、mirror 目前已具备 spec-like payload 和控制面生命周期，但 live SDK 行为断言仍未逐项接入；这些 case 在 `--execute-governance-control-plane` 下仍会明确失败为“behavioral assertions are not implemented”，不会假通过。

## 本轮计划：routing/lane live SDK 行为断言

- [x] 补 RED 测试，约束 routing flow 注册 primary/secondary 两组实例并通过 `x-pole-e2e-route=primary` 请求 primary 路由
- [x] 补 RED 测试，约束 lane flow 注册 blue/green 两组实例并通过 `x-pole-e2e-lane=blue` 请求 blue 泳道
- [x] 增加实例 metadata 断言，验证 Consumer 返回实例命中目标标签
- [x] 将 routing/lane-routing 接入 live e2e：控制面 publish 后注册实例、心跳、consumer get_one、断言 metadata、反注册清理
- [x] 更新旧测试，不再把 routing 当成未实现行为断言的治理 case
- [x] 跑 `cargo test -p e2e_tests -- --nocapture`、`cargo fmt`、`RUSTFLAGS='-D warnings' cargo check --workspace`、`RUSTFLAGS='-D warnings' cargo test --workspace`

## 本轮 routing/lane review

- 已完成：routing/lane-routing 不再只是下发 spec-like payload；打开 `--execute-governance-control-plane` 后会通过真实 Provider/Consumer SDK 流程验证路由结果。
- 已完成：routing 注册 `e2e-route=primary/secondary` 两个实例，route_info 启用 `ruleBasedRouter` 并携带 `x-pole-e2e-route=primary`；断言最终实例 metadata 为 `primary`。
- 已完成：lane-routing 注册 `lane=blue/green` 两个实例，route_info 启用 `laneRouter` 并携带 `x-pole-e2e-lane=blue`；断言最终实例 metadata 为 `blue`。
- 已验证：e2e crate 46 个测试通过；workspace 严格验证通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 46 个测试、doc tests 0 个全部通过，`RUSTFLAGS=-D warnings` 下无 warning。
- 剩余风险：circuitbreaker、fault-detect、lossless、mirror 还缺 live SDK 行为断言；它们目前仍会明确失败为未实现行为断言。

## 本轮计划：circuitbreaker live SDK 行为断言

- [x] 补 RED 测试，约束 circuitbreaker 行为断言必须在 check_resource 被阻断时通过
- [x] 补 RED 测试，要求 circuitbreaker case 在控制面 setup 后进入 SDK 行为断言分支，而不是继续报未实现
- [x] 实现 live circuitbreaker 行为：通过 `DefaultCircuitBreakerAPI` 连续上报两次失败，再 `check_resource` 验证熔断打开
- [x] 将未实现治理 case 测试迁移到 fault-detect，避免继续依赖已完成的 circuitbreaker
- [x] 跑 `cargo test -p e2e_tests -- --nocapture`、`cargo fmt`、`RUSTFLAGS='-D warnings' cargo check --workspace`、`RUSTFLAGS='-D warnings' cargo test --workspace`

## 本轮 circuitbreaker review

- 已完成：circuitbreaker 不再只是下发 spec-like payload；打开 `--execute-governance-control-plane` 后会通过真实 SDK 熔断 API 验证规则生效。
- 已完成：行为流程为：control-plane create/publish 后，`DefaultCircuitBreakerAPI::report_stat` 连续上报 2 次 `RetFail`，再调用 `check_resource`，断言 `pass=false`。
- 已验证：e2e crate 49 个测试通过；workspace 严格验证通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 49 个测试、doc tests 0 个全部通过，`RUSTFLAGS=-D warnings` 下无 warning。
- 剩余风险：fault-detect、lossless、mirror 还缺 live SDK 行为断言；其中 mirror 可以走公开 RouterAPI + 本地 HTTP receiver + shadow 实例注册，fault-detect/lossless 还需要进一步确认公开入口能否证明远程规则下发后的完整行为。

## 本轮计划：mirror live SDK 行为断言

- [x] 补 RED 测试，约束 mirror flow 注册 shadow 实例并携带 `MirrorRequest`
- [x] 补 mirror receiver 原始 HTTP 请求断言，验证 path/body 符合控制面镜像规则
- [x] 将 mirror 接入 live e2e：控制面 publish 后启动本地 shadow HTTP receiver，注册 shadow 实例并通过 `RouterAPI` 触发镜像派发
- [x] 修正 mirror 收尾顺序：route 成功后先等待 shadow receiver 收到请求，再注销 shadow 实例；route 失败时仍 cleanup
- [x] 跑 mirror 定向测试并记录剩余风险

## 本轮 mirror review

- 已完成：mirror 不再只是下发 spec-like payload；打开 `--execute-governance-control-plane` 后会通过真实 `RouterAPI` + SDK Provider 注册 shadow 实例 + 本地 HTTP receiver 验证镜像副本是否发出。
- 已完成：mirror flow 使用稳定 header `x-pole-e2e-mirror=true` 匹配控制面规则，`ProcessRouteRequest.mirror_request` 固定为 `POST /e2e/mirror` 和 body `{"mirror":true}`，shadow 服务名为 `<service>-shadow`。
- 已完成：新增 `finish_mirror_behavior` 收尾约束，避免异步 mirror sender 尚未解析 shadow 实例时就先 deregister；失败路径仍 abort receiver 并执行 cleanup。
- 已验证：`cargo test -p e2e_tests mirror -- --nocapture` 通过，覆盖新增收尾顺序单测、mirror assertion、case execution、control plan 和 flow 测试。
- 剩余风险：fault-detect、lossless 还缺 live SDK 行为断言；workspace 严格验证仍需在剩余能力补齐后统一执行。

## 本轮计划：fault-detect live SDK 行为断言

- [x] 补 RED 测试，约束 fault-detect flow 同时包含规则拉取、目标实例注册、实例拉取和探测请求期望
- [x] 补 fault-detect 探测结果与原始 HTTP 请求断言，检查 `/health` path 和 `x-pole-e2e:<run_id>` header
- [x] 将 fault-detect 接入 live e2e：控制面 publish 后启动本地 HTTP receiver，注册目标实例，触发 SDK `get_service_rule(FaultDetector)` 和 `get_all_instance`，等待 SDK listener 自动探测
- [x] 将未实现治理 case 哨兵迁移到 lossless，避免 fault-detect 完成后测试继续依赖旧状态
- [x] 跑 fault-detect 定向测试并记录剩余风险

## 本轮 fault-detect review

- 已完成：fault-detect 不再只是下发 spec-like payload；打开 `--execute-governance-control-plane` 后会通过真实 SDK discovery/cache/listener 路径验证探测执行。
- 已完成：行为流程为：control-plane create/publish 后，本地绑定 `127.0.0.1:18080`，Provider 注册同名目标实例并心跳，Consumer 拉取 `ServiceRuleType::FaultDetector` 和目标实例，等待 SDK 内置 faultdetect listener 触发 HTTP `GET /health` 探测。
- 已完成：断言本地 receiver 收到的原始 HTTP 请求包含 `/health` 和 `x-pole-e2e:<run_id>`，比直接调用 `execute_fault_detect_plan` 更能覆盖 console 发布、远程 watch、缓存事件、listener reload 和真实探测链路。
- 已验证：`cargo test -p e2e_tests fault_detect -- --nocapture` 通过，覆盖 fault-detect flow、assertion、control plan 和 case execution 测试。
- 剩余风险：fault-detect 规则和本地 receiver 端口当前固定为 `18080`，并行 e2e 或本机端口占用会失败；lossless 还缺 live SDK 行为断言；workspace 严格验证仍需在全部能力补齐后统一执行。

## 本轮计划：lossless live SDK 行为断言

- [x] 补 RED 测试，约束 lossless flow 能拉取 `ServiceRuleType::Lossless` 并提供本地测试实例身份
- [x] 补 lossless 行为断言，检查规则拉取、delay register、readiness/offline 状态码和 register/deregister 计数
- [x] 将 lossless 接入 live e2e：控制面 publish 后拉取 `LosslessRule`，用公开 `LosslessAPI` 运行 delay register 和 endpoint 流程
- [x] 更新 case execution 测试，确认 lossless 不再落入未实现行为断言分支
- [x] 跑 lossless 定向测试并记录剩余风险

## 本轮 lossless review

- 已完成：lossless 不再只是下发 spec-like payload；打开 `--execute-governance-control-plane` 后会通过真实 SDK 规则拉取和公开 `LosslessAPI` 验证优雅上下线行为。
- 已完成：行为流程为：Consumer 拉取 `ServiceRuleType::Lossless` 并 downcast 到 `LosslessRule`，绑定本地 `BaseInstance` 和记录型 `LosslessActionProvider`，调用 `schedule_register` 验证 1 秒 delay 前未注册、delay 后注册，再通过 `serve_endpoint` 验证 `/readiness` 200、`/offline` 200、offline 后 `/readiness` 503。
- 已验证：`cargo test -p e2e_tests lossless -- --nocapture` 通过，覆盖 lossless flow、assertion、control plan 和 case execution 测试，输出无 warning。
- 剩余风险：lossless 的 live e2e 依赖规则发布到 SDK cache 的传播时序；目前通过 `get_service_rule` 的 timeout 等待初始化，后续如真实环境偶发慢传播，可加短轮询重试。

## 本轮最终 e2e review

- 已完成：独立 e2e crate 已覆盖 console/connectivity、service discovery、config center，以及 routing、ratelimit、circuitbreaker、lane-routing、fault-detect、lossless、mirror、auth-security、mock 的控制面 create/publish/cleanup 和 SDK 行为断言路径。
- 已完成：治理类 live e2e 均在 `--execute-governance-control-plane` 打开后执行行为断言；默认关闭时仍明确失败并输出控制面端点，避免对真实环境误操作。
- 已完成：报告输出保持 JSON、Markdown、JUnit XML；case registry 覆盖全部要求能力。
- 已验证：`cargo test -p e2e_tests -- --nocapture` 通过，e2e crate 64 个测试通过，doc tests 0 个。
- 已验证：`cargo fmt` 通过。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo check --workspace` 通过，无 warning。
- 已验证：`PROTOC=/Users/chuntao.liao/Github/pole-io/specification/source/protoc/protoc-darwin-arm64/bin/protoc RUSTFLAGS='-D warnings' cargo test --workspace` 通过，主库 102 个测试、`tests/public_api.rs` 1 个测试、e2e crate 64 个测试、doc tests 0 个全部通过。
- 剩余风险：fault-detect 使用固定 `127.0.0.1:18080` 探测端口；真实 e2e 环境如果并行运行或端口被占用会失败。当前工作区仍包含大量此前主功能改动和 untracked 新目录，提交前需要按最终变更范围统一 review/stage。

## 相关页面

- [[lessons]]
