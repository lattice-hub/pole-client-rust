---
title: 熔断规则编辑 — 设计交接文档
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# 熔断规则编辑 — 设计交接文档

> 适用范围：Pole.IO 治理工作台 → 规则类型 = **熔断（CircuitBreakRule）** 编辑抽屉里的 **「规则」Tab**。
> 本次只关注「规则」Tab 的设计；版本、审计等其它 Tab 暂不在范围内。
> 视觉令牌、共享控件（下拉/开关/分段/折叠卡/悬浮提示）与路由、限流一致，详见 `router-rule-design.md`、`ratelimit-rule-design.md`，本文只描述熔断特有部分。原型见 `index.html`。

---

## 1. 「规则」Tab 整体结构

右侧抽屉，左表单 / 右实时 Spec 双栏。熔断的分段顺序：

```
① 基础信息          —— 名称 / 优先级 / 描述 / 规则标签 / 启用状态
② 服务范围          —— 主调 → 被调 调用关系卡 +「熔断粒度」（规则级）
③ 熔断子规则（多条） —— 每个子规则：多个熔断策略 + 一个恢复策略 + 一个熔断后降级
```

层级关系（关键）：

```
熔断规则
├── 熔断粒度（服务 / 实例 / 接口）—— 规则级，对所有子规则生效
└── 子规则[]（可多个）
    ├── 熔断策略[]（可多个，命中任一策略即触发熔断）
    │   ├── 接口范围[]（可多个接口）
    │   ├── 错误判断条件[]（可多个）
    │   └── 熔断触发条件[]（可多个）
    ├── 恢复策略（该子规则唯一）
    └── 熔断后降级（该子规则唯一）
```

---

## 2. ② 服务范围

- **调用关系卡**：左「主调（发起调用方）」→ 蓝色箭头 →「被调（被熔断的目标服务）」，各含 命名空间 / 服务 下拉。可折叠，收起显示 `主调ns/svc → 被调ns/svc` 摘要。
- **熔断粒度**：分段按钮 `服务 / 实例 / 接口`，**规则级**（对该规则下所有子规则/策略统一生效），默认 `接口`。

---

## 3. ③ 熔断子规则（多条）

顶部「N 条」+ 底部「添加子规则」。每个**子规则**是一张可折叠卡片（蓝色卡头）：

- 折叠态摘要：`{策略数} 个熔断策略 · 熔断 {熔断时长}s · 降级{开/关}`
- 展开后含三块：**A 熔断策略** / **B 恢复策略** / **C 熔断后降级**

### 3.A 熔断策略（子规则内可多个）

「N 个」+「添加熔断策略」。每个**熔断策略**是一张可折叠卡片「熔断策略 [k]：{名称}」，摘要 `{接口数} 个接口 · {错误条件数} 个错误条件 · {触发条件数} 个触发条件`。内部：

- **策略名称**：文本（如 `payment-error-rate`）
- **① 接口范围（多接口）**：列头 协议 / 接口方法 / 接口路径；每行增删 +「添加接口」
  - 协议 `HTTP / gRPC / Dubbo`，方法 `GET/POST/PUT/DELETE/*`，路径为文本
- **② 错误判断条件（多条）**：定义哪些请求算「错误」。列：参数类型 / 匹配类型 / 匹配值 + 增删
  - 参数类型 `返回码 / 异常类型 / 响应体`；匹配类型 `范围匹配 / 完全匹配 / 正则匹配 / 包含`；值如 `500-599`
- **③ 熔断触发条件（多条）**：满足任一即触发熔断。列：类型 / 比较 / 阈值 / 统计周期 / 最小请求数 + 增删
  - 类型 `错误率 / 慢调用比例 / 错误数 / 慢调用数`；比较 `>= / >`
  - 阈值：比例类显示 `%`，计数类显示 `次`
  - 统计周期（秒）、最小请求数（个）

### 3.B 恢复策略（子规则唯一）

- 熔断时长（秒）
- 主动探测（开关）；开启时显示「探测间隔（秒）」，关闭时按熔断时长到期后进入半开试探

### 3.C 熔断后降级（子规则唯一）

- 开关（在子区标题右侧）
- 响应码（如 503）
- 响应头：key/value 列表，可增删（空时显示「暂无响应头」）
- 响应体：文本域（如 `fallback` 或 JSON）

---

## 4. 数据模型（前端 M 结构）

```js
circuit = {
  name, enabled, priority, desc, tags:[],
  scope:{ srcNs, srcSvc, dstNs, dstSvc },     // ② 主调→被调
  granularity: '服务' | '实例' | '接口',       // ② 规则级粒度
  subrules: [                                  // ③ 子规则[]
    {
      strategies: [                            // 熔断策略[]
        {
          name,
          ifaces:    [ { proto, method, path } ],          // 接口范围
          errorConds:[ { param, match, value } ],          // 错误判断条件
          triggers:  [ { type, op, threshold, statSec, minReq } ], // 熔断触发条件
        }
      ],
      recovery: { breakSeconds, probe, probeIntervalSec },  // 恢复策略（唯一）
      fallback: { enabled, code, headers:[{k,v}], body },   // 熔断后降级（唯一）
    }
  ]
}
```

---

## 5. 实时 Spec（CircuitBreakRule，YAML 示例）

```yaml
apiVersion: governance.pole.io/v1
kind: CircuitBreakRule
metadata: { name: spec-check-circuitbreak, enabled: true, priority: 15, labels: [...] }
spec:
  source:      { namespace: spec-governance, service: spec-order }     # 主调
  destination: { namespace: spec-governance, service: spec-payment }   # 被调
  granularity: interface           # service / instance / interface（规则级）
  rules:                           # 子规则[]
    - strategies:                  # 熔断策略[]
        - name: payment-error-rate
          interfaces:
            - { protocol: HTTP, method: GET, path: /api/v1/payments }
          errorConditions:
            - { param: 返回码, op: 范围匹配, value: "500-599" }
          triggers:
            - { type: error-ratio, op: ">=", threshold: 50, statSec: 30, minRequests: 10 }
      recovery: { breakSeconds: 30, activeProbe: false }   # activeProbe=true 时含 probeIntervalSec
      fallback:                                            # enabled=false 时仅 { enabled:false }
        enabled: true
        statusCode: 503
        headers: {}
        body: fallback
```

> 枚举映射（UI 中文 → Spec）：触发类型 `错误率→error-ratio / 慢调用比例→slow-ratio / 错误数→error-count / 慢调用数→slow-count`；粒度 `服务→service / 实例→instance / 接口→interface`。错误判断条件的 `param`/`op` 当前直出中文，落地时可再映射后端枚举。

---

## 6. 校验规则（保存时）

1. 规则名称符合 kebab-case。
2. 至少 1 条**子规则**。
3. 每条子规则：
   - 至少 1 个熔断策略；
   - 每个策略：接口路径非空、错误判断值非空、至少 1 个触发条件，比例类触发的阈值在 0–100 之间；
   - 恢复策略熔断时长 > 0。
4. 错误拦截保存并 Toast，提示按 `子规则[i]·策略[j]` 粒度标注。

---

## 7. 实现要点清单

- [ ] ② 服务范围：主调→被调调用卡（可折叠 + 摘要）+ 熔断粒度分段（规则级）。
- [ ] ③ 子规则增删 + 折叠（蓝色卡头 + 摘要随编辑刷新）。
- [ ] 子规则内：熔断策略增删 + 折叠（嵌套卡片）。
- [ ] 策略内：接口范围多行增删 / 错误判断条件多行增删 / 熔断触发条件多行增删（阈值单位随触发类型在 %/次 间切换）。
- [ ] 恢复策略：熔断时长 + 主动探测开关（开则探测间隔）。
- [ ] 熔断后降级：开关 + 响应码 + 响应头 kv 增删 + 响应体。
- [ ] 右栏实时 Spec（CircuitBreakRule）随编辑刷新，YAML/JSON 切换 + 复制。
- [ ] 保存校验 + 错误高亮 + Toast（子规则·策略 粒度提示）。

## 相关页面
