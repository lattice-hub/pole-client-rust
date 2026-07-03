---
title: 限流规则编辑 — 设计交接文档
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# 限流规则编辑 — 设计交接文档

> 适用范围：Pole.IO 治理工作台 → 规则类型 = **限流（RateLimitRule）** 编辑抽屉里的 **「规则」Tab**。
> 本次只关注「规则」Tab 的设计；版本、审计等其它 Tab 暂不在范围内。
> 视觉体系沿用路由规则同一套 Ant 风格令牌（浅灰底 `#f0f2f5` + 白卡片 + 主色 `#1677ff`），原型见 `index.html`。
> 与路由规则共享的分段样式、控件、悬浮提示规范见 `router-rule-design.md`，本文只描述限流特有部分。

---

## 1. 「规则」Tab 整体结构

右侧抽屉，左表单 / 右实时 Spec 双栏。限流的分段顺序：

```
① 基础信息          —— 名称 / 优先级 / 描述 / 规则标签 / 启用状态
② 作用对象          —— 命名空间 / 服务名称 / 限流模式（单机 · 集群）
③ 限流规则（多条）   —— 每条子规则：匹配接口 / 匹配条件 / 限流方式 / 限流效果
```

- ① 基础信息：与路由一致，仅标识与基本属性，**不含作用域**。
- ② 作用对象：① 与 ③ 之间的独立卡片（不带主调/被调方向，单张普通卡片）。
- ③ 限流规则：**子规则列表**，可增删、可折叠（与鉴权同款折叠卡片模式）。

---

## 2. ② 作用对象

一张普通卡片，三个字段：

| 字段 | 控件 | 取值 / 说明 |
|---|---|---|
| 命名空间 | 下拉 | 规则部署的命名空间 |
| 服务名称 | 下拉 | 规则部署的服务 |
| 限流模式 | 分段按钮 | `单机限流` / `集群限流` —— **驱动 ③ 的限流效果**（见 §3.4）|

> 切到 `集群限流` 时，把所有子规则的触发动作强制设为「快速失败」。

---

## 3. ③ 限流规则（子规则列表）

顶部显示「N 条」，底部「添加规则」。每条子规则是一张**可折叠卡片**：

- **折叠态**：`▸` 箭头 + 「规则 [n]」+ 摘要文本 + 右侧阈值徽标 + 删除按钮（≥2 条时）。
  - 摘要文本：`{接口数} 个接口 · {条件数} 个匹配条件 · {指标} · {动作}`
  - 阈值徽标：请求数 → `120 次 / 1s`（多窗口时 `120 次 / 1s 等 N 窗`）；并发数 → `N 并发`
- **展开态**：四个子区，编号 ①②③④。

### 3.1 子区① 匹配接口（多条）

- 满足**任一接口**的请求才进入限流统计。
- 列头：协议 / 方法 / 匹配类型 / 接口路径。
- 每行一条接口：协议(下拉) + 方法(下拉) + 匹配类型(下拉) + 接口路径(输入) + 删除（≥2 条时）。
- 「添加接口」追加一行。

| 字段 | 控件 | 取值 |
|---|---|---|
| 协议 | 下拉 | `HTTP` / `gRPC` / `Dubbo` |
| 方法 | 下拉 | `GET` / `POST` / `PUT` / `DELETE` / `*` |
| 匹配类型 | 下拉 | `完全匹配` / `前缀匹配` / `正则匹配` / `包含` / `不等于` |
| 接口路径 | 输入(mono) | 如 `/api/v1/payments` |

### 3.2 子区② 匹配条件

- 关系切换 `AND` / `OR`；满足条件的请求才应用限流。
- 条件表格列：参数类型 / 参数键 / 匹配类型 / 匹配值 / 删除。
  - 参数类型：`HEADER` / `QUERY` / `PATH` / `COOKIE` / `METHOD`；参数键随类型联动候选。
  - 匹配类型：同 §3.1。
- 「添加匹配条件」追加一行。

### 3.3 子区③ 限流���式

- **统计指标** 分段：`请求数` / `并发数`。
- 指标 = **请求数** → **多窗口**：
  - 列头：窗口 / 最大请求数。
  - 每行一个窗口：窗口时长(秒) + 最大请求数 + 删除（≥2 条时）。
  - 「添加窗口」追加一行（同一规则可配多个时间窗，如 1s 内 120 次且 60s 内 3000 次）。
- 指标 = **并发数** → 单个「最大并发数」。
- **阈值计算合并** 开关：关闭时按匹配的请求**单独**计算限流（默认关闭）。

### 3.4 子区④ 限流效果（随 ② 限流模式联动）

- **单机限流**：
  - 触发动作可选 `快速失败` / `排队等待`（红/绿结果卡 + banner）。
  - 排队等待时不再额外配置（如需可加最大等待时长，当前不展示）。
  - **不显示**失败处理策略。
- **集群限流**：
  - 触发动作**只支持 `快速失败`**（直接展示红色结果 banner，无切换）。
  - **显示失败处理策略** 分段：`单机限流` / `直接通过` —— 当 Token Server 通信失败或不可用时的降级行为。
- 两种模式都支持 **自定义响应** 开关：开 → JSON 文本框（如 `{"code":429,"msg":"rate limited"}`）；关 → 文案「无自定义响应 — 返回默认 429」。

---

## 4. 数据模型（前端 M.rules 结构）

```js
ratelimit = {
  name, enabled, priority, desc, tags:[],
  scope:{ ns, svc },
  mode: '单机限流' | '集群限流',
  rules: [
    {
      ifaces: [ { proto, method, op, path } ],          // 多接口
      relation: 'AND' | 'OR',
      conditions: [ { type, key, match, value } ],
      metric: '请求数' | '并发数',
      windows: [ { sec, max } ],                         // metric=请求数 时多窗口
      maxConcurrent,                                     // metric=并发数 时
      mergeKey: bool,                                    // 阈值计算合并
      action: '快速失败' | '排队等待',                    // 集群恒为快速失败
      failStrategy: '单机限流' | '直接通过',              // 仅集群
      customResp: bool, respBody: string,                // 自定义响应
    }
  ]
}
```

---

## 5. 实时 Spec（RateLimitRule，YAML 示例）

```yaml
apiVersion: governance.pole.io/v1
kind: RateLimitRule
metadata:
  name: spec-check-ratelimit
  enabled: true
  priority: 15
  labels: [owner:codex, scenario:sample]
spec:
  scope: { namespace: spec-governance, service: spec-gateway }
  mode: cluster                 # ② 限流模式：standalone / cluster
  rules:                        # ③ 子规则数组
    - interfaces:               # ①匹配接口（多条）
        - { protocol: HTTP, method: POST, path: /api/v1/payments, op: 完全匹配 }
      match:                    # ②匹配条件
        relation: AND
        conditions:
          - { param: HEADER, key: x-tenant, op: 完全匹配, value: vip }
      limit:                    # ③限流方式
        metric: requests        # requests / concurrency
        windows:                # metric=requests 时多窗口
          - { windowSec: 1, maxRequests: 120 }
        mergeKey: false
      action: reject            # ④限流效果（集群恒 reject；单机 reject/queue）
      failStrategy: local       # 仅集群输出：local / pass-through
      # customResponse: '{"code":429,...}'   # customResp 开启时输出
```

并发数指标时 `limit` 形如：`{ metric: concurrency, maxConcurrent: 50, mergeKey: false }`（无 windows）。

---

## 6. 校验规则（保存时）

1. 规则名称符合 kebab-case（`^[a-z][a-z0-9-]*$`）。
2. 至少配置 1 条限流子规则。
3. 每条子规则：
   - 接口列表非空，且**不存在空的接口路径**；
   - 指标=请求数 → 窗口列表非空且每个窗口**最大请求数 > 0**；指标=并发数 → **最大并发数 > 0**；
   - **不存在空匹配值**；
   - 开启自定义响应时，响应正文须为**合法 JSON**。
4. 错误拦截保存并 Toast 首条提示（提示中标明第几条子规则）。

---

## 7. 实现要点清单

- [ ] ② 作用对象：命名空间 / 服务 / 限流模式（分段，切集群锁动作=快速失败）。
- [ ] ③ 子规则增删 + 折叠（摘要 + 阈值徽标随编辑实时刷新，路径/数值输入不丢焦点）。
- [ ] 匹配接口多行增删（列头 + 行 + 添加接口）。
- [ ] 匹配条件 AND/OR + 条件行增删 + 参数类型联动键。
- [ ] 限流方式：请求数多窗口增删 / 并发数单值 + 阈值计算合并开关。
- [ ] 限流效果按限流模式分支：单机=快速失败/排队，集群=仅快速失败+失败处理策略；自定义响应开关 + JSON。
- [ ] 右栏实时 Spec（RateLimitRule）随编辑刷新，YAML/JSON 切换 + 复制。
- [ ] 保存校验 + 错误高亮 + Toast。

## 相关页面
