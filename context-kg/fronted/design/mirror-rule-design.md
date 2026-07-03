---
title: 流量镜像规则编辑 — 设计交接文档
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# 流量镜像规则编辑 — 设计交接文档

> 适用范围：Pole.IO 治理工作台 → 规则类型 = **镜像（MirrorRule）** 编辑抽屉里的 **「规则」Tab**。
> 本次只关注「规则」Tab 的设计；版本、审计等其它 Tab 暂不在范围内。
> 视觉体系沿用路由规则同一套 Ant 风格令牌（浅灰底 `#f0f2f5` + 白卡片 + 主色 `#1677ff`），原型见 `index.html`。
> 与路由规则共享的分段样式、控件、悬浮提示规范见 `router-rule-design.md`，本文只描述镜像特有部分。

---

## 1. 「规则」Tab 整体结构

右侧抽屉，左表单 / 右实时 Spec 双栏。镜像规则的分段顺序：

```
① 基础信息          —— 名称 / 优先级 / 描述 / 规则标签 / 启用状态
② 服务范围          —— 主调命名空间 / 主调服务  →  被调命名空间 / 被调服务
③ 镜像规则（多条）   —— 每条子规则：接口范围 / 流量标签 / 镜像执行
```

- ① 基础信息：与路由一致，仅包含标识与基本属性，**不含服务范围**。
- ② 服务范围：表达镜像作用的调用关系，主调与被调都用「命名空间 / 服务」两级下拉。
- ③ 镜像规则：直接进入子规则列表，不展示顶部总览容器或解释性说明块。
- 右栏实时 Spec：随编辑刷新，支持 YAML / JSON 切换和复制。

---

## 2. ② 服务范围（主调 → 被调）

服务范围表达当前镜像规则在哪类调用上生效：

```
主调命名空间 / 主调服务  →  被调命名空间 / 被调服务
```

### 2.1 展开态

一张调用关系卡，左右两侧分别是主调与被调节点：

| 区域 | 字段 | 控件 | 说明 |
|---|---|---|---|
| 主调 | 命名空间 | 下拉 | 可选具体命名空间，也可选 `全部命名空间` |
| 主调 | 服务 | 下拉 | 可选具体服务，也可选 `全部服务` |
| 被调 | 命名空间 | 下拉 | 可选具体命名空间，也可选 `全部命名空间` |
| 被调 | 服务 | 下拉 | 可选具体服务，也可选 `全部服务` |

设计约束：

- 不做「全部服务 / 指定主调」分段按钮；`全部` 是下拉选项，不是模式切换。
- 不输出或展示 `caller.mode`。
- 被调服务属于第 2 部分的服务范围，不放到每条子规则里。
- 主调和被调字段结构保持一致，降低规则理解成本。

### 2.2 折叠态

折叠后标题栏右侧显示一行 mono 摘要：

```
全部命名空间/全部服务  →  spec-governance/spec-order
```

长服务名按路由规则同款策略处理：省略号截断，悬浮显示完整值。

---

## 3. ③ 镜像规则（子规则列表）

镜像规则采用类似限流的多子规则结构：一个服务范围下挂多条镜像子规则。

顶部只显示：

- 标题「镜像规则」
- 说明「在服务范围内配置多条接口 / 标签镜像子规则」
- 子规则数量与启用数量
- 总启用开关

不要在这里放：

- 调用关系总览盒子
- 「启用中的首个镜像目标」卡片
- 解释性提示块
- 从子规则派生出来的全局镜像目标

每条子规则是一张可折叠卡片：

- 折叠态：`镜像子规则 [n]` + 摘要 + 比例徽标 + 删除按钮。
- 摘要文本：`{接口数} 个接口 · {流量标签数} 个流量标签 · {比例}% → {目标服务}`。
- 展开态：三个子区，按「接口范围 → 流量标签 → 镜像执行」排列。

### 3.1 子区① 接口范围

在第 2 部分的被调服务内匹配接口。满足任一接口时进入该子规则。

| 字段 | 控件 | 取值 |
|---|---|---|
| 协议 | 下拉 | `HTTP` / `gRPC` / `Dubbo` |
| 方法 | 下拉 | `GET` / `POST` / `PUT` / `DELETE` / `*` |
| 匹配类型 | 下拉 | `完全匹配` / `前缀匹配` / `正则匹配` / `包含` / `不等于` |
| 接口路径 | 输入框（mono） | 如 `/orders`、`/payment/callback` |

交互：

- 「添加接口」追加一行。
- 至少保留 1 行接口。
- 删除按钮仅在接口数大于 1 时显示。

### 3.2 子区② 流量标签

按请求标签、Header 或其它流量标识筛出需要复制的流量。

| 字段 | 控件 | 示例 |
|---|---|---|
| 关系 | 分段按钮 | `AND` / `OR` |
| 标签键 | 输入框（mono） | `x-traffic-mirror`、`x-gray` |
| 匹配类型 | 下拉 | 与接口匹配类型一致 |
| 标签值 | 输入框 | `true`、`medium` |

交互：

- `AND`：需同时满足全部标签。
- `OR`：满足任一标签即镜像。
- 「添加流量标签」追加一行。
- 至少保留 1 行标签。

### 3.3 子区③ 镜像执行

每条子规则独立设置复制比例与目标服务。

| 字段 | 控件 | 说明 |
|---|---|---|
| 镜像比例 | 数字输入 + `%` 后缀 | 0–100 |
| 目标命名空间 | 下拉 | 镜像流量发送到的命名空间 |
| 目标服务 | 下拉 | 镜像流量发送到的服务 |

布局要求：

- 使用三列以内的自适应布局。
- 目标服务列需要足够宽，避免字段被挤压。
- 不展示时长字段。
- 镜像目标服务只属于当前子规则，不上提到第 3 部分顶部。

执行摘要：

```
当主调方命中第 2 部分范围，并调用 {被调服务} 的上述接口时，
复制 {比例}% 流量到 {目标服务}；原请求仍走主链路。
```

---

## 4. 前端数据模型（M 结构）

```js
mirror = {
  name,
  enabled,
  priority,
  desc,
  tags: [],
  scope: {
    srcNs: '全部命名空间',
    srcSvc: '全部服务',
    dstNs: 'spec-governance',
    dstSvc: 'spec-order'
  },
  on: true,
  subrules: [
    {
      open: true,
      ifaces: [
        { proto: 'HTTP', method: 'GET', op: '前缀匹配', path: '/orders' }
      ],
      tagRelation: 'AND',
      trafficTags: [
        { key: 'x-traffic-mirror', match: '完全匹配', value: 'true' }
      ],
      ratio: 30,
      target: { ns: 'spec-governance', svc: 'spec-shadow' }
    }
  ]
}
```

兼容迁移要求：

- 旧数据里的 `callerMode` 只用于迁移到 `srcNs/srcSvc`，当前 UI 和 Spec 不再保留。
- 旧数据里的单条镜像字段应折算为 `subrules[0]`。
- 子规则里不保留 `callee`；被调统一来自 `scope.dstNs/dstSvc`。

---

## 5. 实时 Spec（MirrorRule，YAML 示例）

```yaml
apiVersion: governance.pole.io/v1
kind: MirrorRule
metadata:
  name: spec-check-traffic-mirror-20260612
  enabled: true
  priority: 20
  labels:
    - owner:codex
    - scenario:sample
spec:
  serviceRange:
    caller:
      namespace: 全部命名空间
      service: 全部服务
    callee:
      namespace: spec-governance
      service: spec-order
  description: 流量镜像样例：全部主调服务调用订单服务时，将中等灰度 header 的 30% 流量镜像到 shadow 服务
  enabled: true
  rules:
    - interfaces:
        - { protocol: HTTP, method: GET, path: /orders, op: 前缀匹配 }
      trafficLabels:
        relation: AND
        labels:
          - { key: x-traffic-mirror, op: 完全匹配, value: "true" }
      mirror:
        percent: 30
        target:
          namespace: spec-governance
          service: spec-shadow
    - interfaces:
        - { protocol: HTTP, method: POST, path: /payment/callback, op: 完全匹配 }
      trafficLabels:
        relation: AND
        labels:
          - { key: x-gray, op: 完全匹配, value: medium }
      mirror:
        percent: 10
        target:
          namespace: spec-governance
          service: spec-shadow-canary
```

字段映射：

| Spec 字段 | 来源 |
|---|---|
| `metadata` | ① 基础信息 |
| `spec.serviceRange.caller` | ② 服务范围.主调 |
| `spec.serviceRange.callee` | ② 服务范围.被调 |
| `spec.enabled` | ③ 镜像规则总启用开关 |
| `spec.rules[].interfaces` | 子规则.接口范围 |
| `spec.rules[].trafficLabels` | 子规则.流量标签 |
| `spec.rules[].mirror.percent` | 子规则.镜像比例 |
| `spec.rules[].mirror.target` | 子规则.镜像目标服务 |

---

## 6. 校验规则（保存时）

保存前全量校验，任一不通过则拦截保存、Toast 提示首条错误，并高亮对应字段：

1. 规则名称符合 kebab-case（`^[a-z][a-z0-9-]*$`）。
2. 至少配置 1 条镜像子规则。
3. 主调命名空间、主调服务、被调命名空间、被调服务不能为空；`全部命名空间` / `全部服务` 是合法值。
4. 每条子规则：
   - 接口列表非空，且不存在空接口路径；
   - 流量标签列表非空；
   - 不存在空标签键或空标签值；
   - 镜像比例必须在 0–100 之间；
   - 镜像目标服务不能为空。

校验通过 → 写入持久化（原型用 localStorage）→ Toast「已保存并下发到数据面」→ 关闭抽屉。

---

## 7. 视觉与布局要点

- 延续 Ant Design 企业控制台风格：浅灰背景、白色卡片、发丝边框、蓝色主操作。
- 第 2 部分服务范围复用路由/熔断的调用关系卡片，但主调与被调都允许 `全部` 下拉值。
- 第 3 部分不需要顶部视觉总览，直接展示子规则列表，减少重复解释。
- 子规则内部密度要比普通表单更宽松，避免把接口、标签、比例和目标服务挤在同一行。
- 折叠摘要要优先展示可判断行为的核心信息：接口数、标签数、比例、目标服务。
- Spec 右栏保持深色代码面板，YAML / JSON 切换和复制行为与其它规则一致。

---

## 8. 实现要点清单

- [ ] ② 服务范围：主调 / 被调均为「命名空间 + 服务」下拉，包含 `全部命名空间` 与 `全部服务`。
- [ ] 删除或避免出现主调模式切换，不展示 `caller.mode`。
- [ ] ③ 镜像规则直接显示子规则列表，不出现顶部总览容器、目标卡或解释块。
- [ ] 子规则增删 + 折叠，摘要和比例徽标随编辑实时刷新。
- [ ] 接口范围支持多行增删。
- [ ] 流量标签支持 AND/OR 与多行增删。
- [ ] 镜像执行只包含比例、目标命名空间、目标服务。
- [ ] 右栏实时 Spec 输出 `serviceRange.caller / serviceRange.callee / rules[].mirror.target`。
- [ ] 保存校验覆盖服务范围、接口、标签、比例和目标服务。

## 相关页面
