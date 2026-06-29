---
title: 知识库 Schema
tags: [meta, schema]
links: [index, log]
updated: 2026-06-17
sources: 1
---

# 知识库 Schema

本知识库使用 `context-kg/` 三域结构承载项目长期知识，`tasks/` 只记录任务过程。

## 目录职责

- `_meta/`：知识库 schema、入口索引和变更日志。
- `business/`：业务术语、领域实体、业务规则、功能档案和产品决策。
- `technical/`：架构、模块边界、接口契约、配置、缓存、部署、技术约定、ADR 和技术债。
- `quality/`：测试策略、测试用例、自动化测试背景、缺陷复盘和回归知识。
- `tasks/`：任务计划、进度、review 和 lessons。

## 页面约定

每个非临时 Markdown 页面必须包含 frontmatter：

```yaml
---
title: 页面标题
tags: [tag1, tag2]
links: [page-name]
updated: YYYY-MM-DD
sources: 0
---
```

`title` 必须匹配 H1，`links` 使用不带 `.md` 的页面 basename，`sources` 表示该页面使用的源码文件、配置、测试、规范或命令输出数量。

## 链接约定

页面需要同步维护三类链接：

- frontmatter `links`
- 正文中的 wiki 链接
- 末尾 `## 相关页面`

frontmatter `links` 与 `## 相关页面` 必须包含相同页面名，且链接目标必须存在。

## 代码反向建库约定

从代码反向生成知识页时，只写源码、测试、配置、spec、manifest 或命令输出能证明的事实。测试和 examples 可以证明行为被覆盖，但不能直接证明产品意图；不确定意图应写为开放问题或省略。

长期架构、技术方案、缓存/API/存储设计默认进入 `technical/` 或 `technical/adr/`；业务能力摘要进入 `business/`；测试体系和验证背景进入 `quality/`。

## 相关页面

- [[index]]
- [[log]]
