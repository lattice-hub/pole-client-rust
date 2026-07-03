---
title: 探测规则编辑 — 设计交接文档
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# 探测规则编辑 — 设计交接文档

> 适用范围：Pole.IO 治理工作台 → 规则类型 = **探测（FaultDetectRule）** 编辑抽屉里的 **「规则」Tab**。
> 本次只关注「规则」Tab 的设计；版本、审计等其它 Tab 暂不在范围内。
> 视觉令牌、共享控件（抽屉、基础信息、分段按钮、折叠卡、Spec 预览、Toast）与路由、限流、熔断一致，详见 `router-rule-design.md`、`ratelimit-rule-design.md`、`circuit-rule-design.md`。原型见 `index.html`。

---

## 1. 设计目标

探测规则不是熔断策略，也不是健康治理动作配置。它的核心结构是：

```
一个被探测对象
└── 多条探测规则
    ├── 探测协议：HTTP / TCP / UDP
    ├── 启停状态
    ├── 探测周期：间隔 / 超时
    ├── 端口策略：实例协议端口 / 指定探测端口
    └── 协议载荷
        ├── HTTP：方法 / URL / Headers
        └── TCP、UDP：发送内容 / 接收内容 / 匹配方式
```

关键约束：

- 探测协议只允许 `HTTP / TCP / UDP`，不要复用接口规则里的 `HTTP / gRPC / Dubbo`。
- 同一个服务可以配置多条探测规则，每条规则独立启停、独立配置协议和周期。
- 端口默认不需要填写，应表达为「实例协议端口」；只有用户需要使用独立探测端口时，才切换到「指定探测端口」并填写端口。
- TCP/UDP 探测必须支持请求报文与接收报文匹配，不能只展示协议和端口。
- 不要加入健康阈值、失败处置、实例摘除、熔断恢复等概念，除非后端契约或用户明确要求。

---

## 2. 「规则」Tab 整体结构

右侧抽屉，左表单 / 右实时 Spec 双栏。探测规则的分段顺序：

```
① 基础信息          —— 名称 / 优先级 / 描述 / 规则标签 / 启用状态
② 被探测对象        —— 命名空间 / 服务名称
③ 探测规则（多条）  —— 每条规则：协议 / 状态 / 周期 / 端口策略 / 协议载荷
```

- ① 基础信息：与其它规则一致，仅放规则标识与通用属性，**不放服务作用域**。
- ② 被探测对象：单个服务对象，不展示主调 → 被调调用关系。
- ③ 探测规则：可增删的折叠卡片列表。卡片标题展示摘要，展开后按「基础调度 / 端口策略 / 协议载荷」组织。

---

## 3. ② 被探测对象

一张普通卡片，字段如下：

| 字段 | 控件 | 说明 | 必填 |
|---|---|---|---|
| 被探测命名空间 | 下拉 | 服务所在命名空间 | 是 |
| 被探测服务 | 下拉 | 需要被探测的服务 | 是 |

标题栏右侧显示 mono 摘要：

```text
spec-governance/spec-payment
```

长命名空间或服务名按已有规则支持省略号截断与悬浮提示。

---

## 4. ③ 探测规则列表

顶部显示「N 条」，底部有「添加探测规则」。每条规则是一张可折叠卡片。

### 4.1 折叠态

卡片标题：

```text
探测规则 [n]    HTTP · 实例协议端口 · 5s 间隔 / 2s 超时 · 启用
```

摘要格式：

```text
{协议} · {端口摘要} · {间隔}s 间隔 / {超时}s 超时 · {启用|禁用}
```

端口摘要：

- `实例协议端口`：端口字段为空，运行时自动选择目标实例中与探测协议匹配的注册端口。
- `指定端口 8080`：用户显式覆盖探测端口。

删除按钮位于卡片右侧。若只剩 1 条规则，可继续允许删除后由校验拦截，也可以禁用删除；当前原型采用保存时校验「至少 1 条」。

### 4.2 展开态：基础调度

第一行四个字段：

| 字段 | 控件 | 取值 / 说明 |
|---|---|---|
| 探测协议 | 分段按钮 | `HTTP` / `TCP` / `UDP` |
| 状态 | 分段按钮 | `启用` / `禁用` |
| 间隔 | 步进器 | 秒，必须 > 0 |
| 超时 | 步进器 | 秒，必须 > 0 |

协议切换行为：

- 切到 `HTTP`：显示 HTTP 方法、URL、Headers；保留或初始化 `method=GET`、`url=/healthz`、`headers=[]`。
- 切到 `TCP` 或 `UDP`：隐藏 HTTP 方法、URL、Headers；显示报文匹配区；保留或初始化 `payload`。
- 切换协议不应清空用户已填写的另一协议配置，除非实现层需要做显式迁移；至少不要让页面报错。

### 4.3 展开态：端口策略

端口策略是独立子区，标题为「端口策略」，说明文案：

```text
默认按实例注册的协议端口探测，只有特殊探测端口才需要覆盖。
```

字段：

| 字段 | 控件 | 说明 |
|---|---|---|
| 端口来源 | 分段按钮 | `实例协议端口` / `指定探测端口` |
| 探测端口 | 步进器 | 仅当端口来源为 `指定探测端口` 时显示，范围 1–65535 |
| 当前策略 | 说明框 | 仅当端口来源为 `实例协议端口` 时显示 |

默认状态：

- 端口来源为 `实例协议端口`。
- 不展示端口输入框。
- Spec 输出 `portMode: INSTANCE_PROTOCOL_PORT`。
- 保存校验不要求端口值。

切到 `指定探测端口`：

- 展示「探测端口」输入。
- 默认可填入 `8080` 或保持用户上一次输入。
- Spec 输出 `portMode: CUSTOM` 与 `port: {number}`。
- 保存校验要求端口在 1–65535。

### 4.4 HTTP 协议载荷

当 `探测协议 = HTTP` 时，展示 HTTP 子表单：

| 字段 | 控件 | 取值 / 说明 |
|---|---|---|
| 方法 | 下拉 | `GET` / `POST` / `PUT` / `DELETE` / `*` |
| URL | 输入框 | 如 `/healthz`，必填 |
| Headers | key/value 列表 | 可增删；非空行必须同时填写 key 与 value |

Headers 子区标题为「Headers」，说明「随探测请求发送的请求头」。底部显示「添加标签」按钮与当前 Header 数量。

### 4.5 TCP / UDP 报文匹配

当 `探测协议 = TCP` 或 `UDP` 时，展示「报文匹配」子区。

子区说明：

- TCP：`建立连接后发送请求内容，再读取响应内容比对。`
- UDP：`发送 UDP 数据报后读取返回数据报内容比对。`

字段：

| 字段 | 控件 | 取值 / 说明 | 必填 |
|---|---|---|---|
| 匹配方式 | 下拉 | `完全匹配` / `包含匹配` / `正则匹配` | 是 |
| 发送内容 | 多行文本 | 请求报文，可为空；例如 `PING\n` | 否 |
| 接收内容 | 多行文本 | 期望响应报文；例如 `PONG` | 是 |

交互要求：

- 发送内容允许为空，用于只建连、只收包或服务端主动响应的场景。
- 接收内容不能为空，因为健康判断需要可比对的期望响应。
- 文本域使用 mono 字体，允许输入换行。实现层需要保留 `\n`。
- 正则匹配只负责收集表达式，原型不在前端执行正则校验；落地时可在保存前做基础正则合法性检查。

---

## 5. 前端数据模型

当前原型中 `M` 的探测规则结构建议如下：

```js
fault = {
  name,
  enabled,
  priority,
  desc,
  tags: [],
  scope: {
    ns: 'spec-governance',
    svc: 'spec-payment',
  },
  rules: [
    {
      proto: 'HTTP' | 'TCP' | 'UDP',
      enabled: true,
      interval: 5,
      timeout: 2,
      port: '',                 // '' 表示实例协议端口；非空表示指定探测端口

      // HTTP only
      method: 'GET',
      url: '/healthz',
      headers: [{ k: 'x-seed', v: 'true' }],

      // TCP / UDP only
      payload: {
        request: 'PING\n',
        response: 'PONG',
        match: '完全匹配' | '包含匹配' | '正则匹配',
      },

      open: true,               // UI 折叠状态
    }
  ]
}
```

实现注意：

- `port` 保持字符串更方便表达空值；生成 Spec 时再转 number。
- `headers` 与 `payload` 可以在协议切换时懒初始化，避免旧数据打开时报错。
- `open` 是纯 UI 状态，不进入 Spec。

---

## 6. 实时 Spec（FaultDetectRule，YAML 示例）

```yaml
apiVersion: governance.pole.io/v1
kind: FaultDetectRule
metadata:
  name: spec-check-faultdetect
  enabled: true
  priority: 5
  labels:
    - owner:codex
    - scenario:sample
spec:
  scope:
    namespace: spec-governance
    service: spec-payment
  description: spec FaultDetectRule HTTP sample
  rules:
    - name: probe-1
      enabled: true
      protocol: HTTP
      intervalSec: 5
      timeoutSec: 2
      portMode: INSTANCE_PROTOCOL_PORT
      http:
        method: GET
        path: /healthz
        headers:
          x-seed: "true"

    - name: probe-2
      enabled: true
      protocol: TCP
      intervalSec: 10
      timeoutSec: 3
      portMode: INSTANCE_PROTOCOL_PORT
      payload:
        request: "PING\n"
        expectedResponse: PONG
        match: 包含匹配

    - name: probe-3
      enabled: true
      protocol: UDP
      intervalSec: 10
      timeoutSec: 3
      portMode: CUSTOM
      port: 19000
      payload:
        request: "PING\n"
        expectedResponse: PONG
        match: 完全匹配
```

字段映射：

| UI 字段 | Spec 字段 |
|---|---|
| ② 被探测命名空间 / 服务 | `spec.scope.namespace/service` |
| 探测规则序号 | `spec.rules[].name = probe-{n}` |
| 探测协议 | `spec.rules[].protocol` |
| 状态 | `spec.rules[].enabled` |
| 间隔 / 超时 | `intervalSec / timeoutSec` |
| 端口来源 = 实例协议端口 | `portMode: INSTANCE_PROTOCOL_PORT`，不输出 `port` |
| 端口来源 = 指定探测端口 | `portMode: CUSTOM`，输出 `port` |
| HTTP 方法 / URL / Headers | `http.method/path/headers` |
| TCP/UDP 发送内容 / 接收内容 / 匹配方式 | `payload.request/expectedResponse/match` |

> 枚举落地建议：UI 可保留中文展示；进入真实后端协议时，`完全匹配 / 包含匹配 / 正则匹配` 建议映射为 `EXACT / CONTAINS / REGEX` 等稳定枚举。

---

## 7. 保存校验规则

保存前全量校验，任一不通过则拦截保存、Toast 提示首条错误，并高亮对应字段。

通用校验：

1. 规则名称符合 kebab-case（`^[a-z][a-z0-9-]*$`）。
2. 至少配置 1 条探测规则。

每条探测规则：

1. 探测协议必须是 `HTTP`、`TCP` 或 `UDP`。
2. 间隔必须 > 0。
3. 超时必须 > 0。
4. 端口为空合法，表示实例协议端口。
5. 端口非空时，必须在 1–65535。
6. HTTP：
   - URL 不能为空；
   - Headers 中不允许出现空 key 或空 value。
7. TCP / UDP：
   - 匹配方式必须是 `完全匹配`、`包含匹配`、`正则匹配` 之一；
   - 接收内容不能为空；
   - 发送内容可以为空。

错误提示建议：

```text
探测规则[2] 接收内容不能为空
探测规则[1] 端口需在 1–65535 之间，留空表示实例协议端口
探测规则[3] 探测协议必须是 HTTP、TCP 或 UDP
```

---

## 8. 实现要点清单

- [ ] ① 基础信息沿用现有规则通用表单，不放作用域字段。
- [ ] ② 被探测对象：命名空间 / 服务下拉，标题栏摘要显示 `ns/service`。
- [ ] ③ 探测规则列表：多条卡片增删、折叠、摘要实时刷新。
- [ ] 探测协议使用独立枚举 `HTTP/TCP/UDP`，不要复用 `PROTO=['HTTP','gRPC','Dubbo']`。
- [ ] 协议切换时条件渲染：
  - [ ] HTTP 显示方法 / URL / Headers。
  - [ ] TCP/UDP 显示报文匹配。
- [ ] 端口策略用分段按钮表达：
  - [ ] 默认 `实例协议端口`，不显示端口输入。
  - [ ] 切到 `指定探测端口` 后显示端口步进器。
- [ ] TCP/UDP 报文匹配：
  - [ ] 匹配方式下拉。
  - [ ] 发送内容多行文本，允许为空。
  - [ ] 接收内容多行文本，必填。
- [ ] 右栏实时 Spec（FaultDetectRule）随编辑刷新，YAML/JSON 切换 + 复制。
- [ ] 保存校验 + 错误高亮 + Toast。

---

## 9. 与其它规则类型的边界

- 与 **熔断** 区分：探测规则不配置熔断时长、恢复策略、降级响应、错误率阈值。
- 与 **限流** 区分：探测规则不配置统计窗口、请求数、并发数、快速失败/排队。
- 与 **路由** 区分：探测规则不配置匹配条件组和目标分组权重。
- 与 **接口匹配类规则** 区分：探测协议是 `HTTP/TCP/UDP`，不是 `HTTP/gRPC/Dubbo`。

这份文档的目标是让后续实现优先还原真实探测规则的配置结构，而不是把它做成泛化治理策略编辑器。

## 相关页面
