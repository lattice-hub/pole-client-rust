---
title: Mock 规则编辑 — 设计交接文档
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# Mock 规则编辑 — 设计交接文档

> 适用范围：Pole.IO 治理工作台 → 规则类型 = **Mock（MockRule）** 编辑抽屉里的 **「规则」Tab**。
> 本文档只描述页面交互、前端视图组织、页面预览映射与校验规则，供 Codex 实现前端原型或页面优化参考。
> **重要边界：本设计不调整底层数据结构、后端 schema 或协议契约。** 文中出现的 `rules[]`、`interface`、`match`、`response` 只用于当前原型的前端视图组织和右侧预览映射；真实提交时应由实现侧适配到现有后端契约。
> 视觉体系沿用当前 `index.html` 的 Ant Design 控制台风格：浅灰页面底、白色抽屉卡片、蓝色主按钮、状态色 pill、右侧实时 Spec 预览。

---

## 1. 核心结论

Mock 规则不是单条全局接口规则，而是**一个服务下挂多条 Mock 子规则**。

页面组织应固定为：

```text
Mock 大规则
├── ① 基础信息（通用）
├── ② Mock 服务（大规则层）
└── ③ Mock 子规则（多条）
    ├── 子规则 [1]
    │   ├── 选择接口
    │   ├── 流量匹配
    │   └── Mock 响应结果
    ├── 子规则 [2]
    │   ├── 选择接口
    │   ├── 流量匹配
    │   └── Mock 响应结果
    └── ...
```

关键交互约束：

1. 服务信息在大规则层配置，Mock 子规则挂在服务下面。
2. 一个 Mock 大规则下可以有多条 Mock 子规则。
3. 每条 Mock 子规则固定按 `选择接口 → 流量匹配 → Mock 响应结果` 的顺序配置。
4. Mock 子规则不需要配置 Mock 比例；命中接口和流量匹配条件后直接返回配置的 Mock 响应。
5. `Mock 响应结果` 内部按 `响应 Code / 响应延迟`、`响应 Header`、`响应 Body`、`最终返回预览` 组织。
6. `响应 Code` 可以是字符串，不限制为 HTTP 数字状态码。
7. `响应 Header` 是通用 Key-Value 列表，不绑定固定 HTTP 或固定 `Content-Type` 控件。
8. `响应 Body` 是主要内容编辑区，未满 15 行时按当前内容自适应高度；超过 15 行后内部滚动，并提供全屏编辑入口。
9. 页面优化只调整前端交互组织和预览映射，不要描述成底层协议或后端数据结构调整。

---

## 2. 「规则」Tab 整体结构

右侧抽屉，左表单 / 右实时预览双栏。

```text
「规则」Tab
├── 左侧：表单区
│   ├── ① 基础信息
│   ├── ② Mock 服务
│   └── ③ Mock 子规则
└── 右侧：实时规则预览

底部固定操作栏：校验摘要 + 取消 / 保存并下发
```

说明：

- ① 基础信息沿用当前规则编辑器通用结构，放规则名称、启用状态、优先级、描述、标签等通用属性。
- ② Mock 服务是 Mock 规则的服务级作用域，放命名空间和服务名。
- ③ Mock 子规则是 Mock 的专属编辑区，支持多条子规则，每条子规则独立配置接口、流量匹配和响应结果。

---

## 3. ② Mock 服务

标题：

```text
Mock 服务
Mock 规则作用的服务对象
```

字段：

| 字段 | 控件 | 说明 | 必填 |
|---|---|---|---|
| 命名空间 | 下拉或输入 | Mock 规则所属服务的命名空间 | 是 |
| 服务名称 | 下拉或输入 | Mock 规则作用的服务 | 是 |

辅助说明：

```text
该服务下可以配置多条 Mock 子规则；每条子规则独立选择接口、流量匹配条件和 Mock 响应结果。
```

服务信息只在大规则层出现。不要在每条 Mock 子规则里重复配置服务。

---

## 4. ③ Mock 子规则

标题：

```text
Mock 子规则
服务下可配置多条 Mock 子规则，每条独立接管接口、匹配流量并返回响应
```

顶部辅助卡片：

```text
服务级作用域
当前服务 {namespace}/{service} 下的请求，会依次进入 Mock 子规则判断。

子规则结构
每条子规则固定按 选择接口 → 流量匹配 → Mock 响应结果 配置，不再额外配置流量百分比。
```

子规则列表：

- 每条 Mock 子规则是一张可折叠卡片。
- 卡片标题格式为 `Mock 子规则 [n]`。
- 卡片摘要展示接口和流量匹配关系，例如：

```text
GET /mock/orders · 需同时满足全部条件
POST /mock/payment · 满足任一条件即 Mock
```

添加按钮：

```text
添加 Mock 子规则
```

删除规则：

- 多于 1 条子规则时允许删除。
- 如果只剩 1 条，删除按钮可以隐藏或禁用；保存时仍校验至少 1 条 Mock 子规则。

---

## 5. Mock 子规则内部结构

每条子规则展开后按三段展示：

```text
子规则 [n]
├── 1. 选择接口
├── 2. 流量匹配
└── 3. Mock 响应结果
```

### 5.1 选择接口

目的：确定这条 Mock 子规则接管哪个接口。

字段：

| 字段 | 控件 | 说明 | 必填 |
|---|---|---|---|
| 协议 | 下拉或分段选择 | 复用当前接口协议集合，例如 `HTTP / gRPC / Dubbo` | 是 |
| 方法 | 下拉 | 例如 `GET / POST / PUT / DELETE` | 是 |
| 路径匹配 | 下拉 | 例如 `完全匹配 / 前缀匹配 / 正则匹配` | 是 |
| 接口路径 | 输入框 | 例如 `/mock/orders` | 是 |

交互要求：

- 接口路径使用等宽字体，便于阅读接口字符串。
- 修改方法或路径后，卡片摘要同步刷新。
- 接口路径不能为空。

### 5.2 流量匹配

目的：定义哪些请求会触发当前 Mock 子规则。

字段：

| 字段 | 控件 | 说明 | 必填 |
|---|---|---|---|
| 匹配关系 | 分段按钮 | `AND` / `OR` | 是 |
| 匹配条件 | 条件表格 | 参数类型、参数键、匹配类型、匹配值 | 至少 1 条 |

匹配关系说明：

- `AND`：需同时满足全部条件。
- `OR`：满足任一条件即 Mock。

条件表格列：

| 列 | 控件 | 示例 |
|---|---|---|
| 参数类型 | 下拉 | `HEADER / QUERY / PATH / COOKIE / METHOD` |
| 参数键 | 输入或下拉 | `x-mock`、`region`、`uid` |
| 匹配类型 | 下拉 | `完全匹配 / 包含匹配 / 前缀匹配 / 正则匹配` |
| 匹配值 | 输入 | `orders`、`payment` |
| 操作 | 删除按钮 | 删除该条件 |

添加按钮：

```text
添加匹配条件
```

交互要求：

- 不需要 Mock 比例。
- 条件不能为空。
- 匹配值不能为空。
- 匹配条件是触发 Mock 的唯一流量条件，不要再加入独立的比例开关或抽样百分比。

---

## 6. 3. Mock 响应结果

Mock 响应结果是子规则的第三段。它需要表达返回内容，但视觉上不要做成卡片堆叠。

推荐结构：

```text
3. Mock 响应结果
┌ 单一响应面板 ─────────────────────────┐
│ 1 响应 Code / 延迟                    │
│   [响应 Code]        [响应延迟 ms]     │
│   提示：执行顺序：先等待 Nms，再返回响应 Code X。 │
│──────────────────────────────────────│
│ 2 响应 Header                         │
│   Key-Value 列表                       │
│──────────────────────────────────────│
│ 3 响应 Body                           │
│   自适应正文编辑区 + 全屏编辑入口       │
└──────────────────────────────────────┘

最终返回预览
┌ Code X · delay Nms ──────────────────┐
│ Code: X                               │
│ header-key: header-value              │
│                                      │
│ body...                               │
└──────────────────────────────────────┘
```

视觉要求：

- 外层只保留一个响应面板。
- Code、Header、Body、延迟之间用细分隔线和小标题表达层级。
- 不要为 Code、Header、Body、延迟分别做独立白底卡片。
- 不要再展示额外的 `响应内容` 总标题或说明行。

### 6.1 响应 Code / 延迟

Code 和延迟左右并排：

| 字段 | 控件 | 说明 | 必填 |
|---|---|---|---|
| 响应 Code | 输入框 | 可以是字符串，例如 `200`、`SUCCESS`、`MOCK_OK` | 是 |
| 响应延迟 | 数字输入 | 单位 ms，必须 >= 0 | 是 |

提示信息放在这两个输入框下面：

```text
执行顺序：先等待 {delay}ms，再返回响应 Code {code}。
```

注意：

- 这条提示是 Code / 延迟配置的行下提示。
- 不要把它放进最终返回预览主体里。
- 不要和输入控件混在同一行。

Code 规则：

- Code 可以是字符串。
- 不要绑定成 HTTP 数字状态码。
- 不要做 `100–599` 范围校验。
- 只校验不能为空。
- 预览映射中保留用户填写的原始字符串。

不需要 Reason：

- 不展示 `Reason` 输入。
- 不在预览映射中输出 `reason`。
- 旧草稿里如果存在 `reason`，可在前端归一化时清理，但不要进入 UI 或预览。

### 6.2 响应 Header

Header 是通用响应头 Key-Value 列表。

字段：

| 字段 | 控件 | 说明 |
|---|---|---|
| Header 名 | 输入框 | 例如 `content-type`、`x-mock-source` |
| Header 值 | 输入框 | 例如 `application/json`、`rule-editor` |
| 操作 | 删除按钮 | 删除该 Header |

添加按钮：

```text
添加响应头
```

交互要求：

- Header 不绑定固定 HTTP 或固定 `Content-Type` 控件。
- `Content-Type` 只是普通 Header 行，由业务自己填写。
- 如果 Header 值已填写但 Header 名为空，需要校验拦截。
- `Content-Type` 可以作为普通 Header 影响 Body 校验：
  - 当普通 Header 中存在 `content-type` 且值包含 `json` 时，Body 非空则必须是合法 JSON。
  - 当 Content-Type 不包含 JSON 时，不强制 Body 是 JSON。

### 6.3 响应 Body

Body 是主要内容编辑区，需要占满可用宽度。

控件：

```text
响应 Body
自适应高度，超过 15 行后内部滚动
[Content-Type pill] [全屏编辑]
[textarea]
```

交互要求：

1. 普通态 textarea 从 1 行高度开始。
2. 未超过 15 行时，高度必须贴合当前内容高度。
3. 不要被外层布局、宽编辑区样式或旧 `min-height` 撑成大文本框。
4. 超过 15 行后，textarea 高度固定在 15 行上限，并在输入框内部滚动。
5. 提供 `全屏编辑` 按钮。
6. 全屏编辑器内容与普通 textarea 使用同一个字段，实时同步到当前 Mock 子规则。
7. 全屏关闭后，普通 textarea 重新按内容适配高度。

全屏编辑入口：

```text
全屏编辑
```

全屏编辑器标题：

```text
响应 Body 全屏编辑
Mock 子规则 [n]
```

底部按钮：

```text
完成
```

辅助说明：

```text
编辑内容会实时同步到当前 Mock 子规则。
```

### 6.4 最终返回预览

最终返回预览放在响应面板下方，作为汇总展示。

标题：

```text
最终返回预览
按响应内容和延迟汇总
```

说明：

```text
命中这条子规则后，客户端会收到下面的响应。
```

预览格式：

```text
Code: {code}
{header-key}: {header-value}

{body}
```

预览条顶部展示：

```text
Code {code}    delay {delay}ms
```

注意：

- 最终返回预览只做结果汇总。
- 执行顺序说明不要放在这里。
- 不要在预览里输出 `Reason`。

---

## 7. 前端视图模型

> 仅用于当前原型的页面状态组织和预览映射，不代表后端契约调整。

推荐视图模型：

```js
{
  name: 'spec-check-traffic-mock-20260612',
  enabled: true,
  priority: 30,
  desc: '流量 Mock 样例',
  tags: ['owner:codex', 'scenario:sample'],
  scope: {
    ns: 'spec-governance',
    svc: 'spec-gateway'
  },
  rules: [
    {
      open: true,
      iface: {
        proto: 'HTTP',
        method: 'GET',
        path: '/mock/orders',
        op: '完全匹配'
      },
      relation: 'AND',
      conditions: [
        {
          type: 'HEADER',
          key: 'x-mock',
          match: '完全匹配',
          value: 'orders'
        }
      ],
      delay: 0,
      code: '200',
      headers: [
        { k: 'content-type', v: 'application/json' }
      ],
      body: '{"orderId":"mock-001","status":"PAID"}'
    }
  ]
}
```

字段说明：

| 字段 | 说明 |
|---|---|
| `scope` | Mock 大规则的服务作用域 |
| `rules[]` | Mock 子规则列表 |
| `rules[].iface` | 当前子规则接管的接口 |
| `rules[].relation` | 流量匹配关系，`AND` 或 `OR` |
| `rules[].conditions[]` | 流量匹配条件 |
| `rules[].delay` | 返回延迟，单位 ms |
| `rules[].code` | 响应 Code，字符串 |
| `rules[].headers[]` | 响应 Header Key-Value |
| `rules[].body` | 响应 Body 字符串 |

归一化兼容：

- 如果旧草稿还是单条 Mock 字段，可在前端归一化为 `rules[0]`。
- 旧草稿里的 `ratio`、`ratioPercent`、`reason` 应在归一化时清理，不进入 UI 或预览。
- 旧草稿里的固定 `Content-Type` 字段应归一化为普通 Header 行。

---

## 8. 右侧预览映射

右侧预览可按以下结构展示：

```js
{
  apiVersion: 'governance.pole.io/v1',
  kind: 'MockRule',
  metadata: {
    name: 'spec-check-traffic-mock-20260612',
    enabled: true,
    priority: 30,
    labels: ['owner:codex', 'scenario:sample']
  },
  spec: {
    scope: {
      namespace: 'spec-governance',
      service: 'spec-gateway'
    },
    description: '流量 Mock 样例',
    rules: [
      {
        name: 'mock-subrule-1',
        interface: {
          protocol: 'HTTP',
          method: 'GET',
          path: '/mock/orders',
          op: '完全匹配'
        },
        match: {
          relation: 'AND',
          conditions: [
            {
              param: 'HEADER',
              key: 'x-mock',
              op: '完全匹配',
              value: 'orders'
            }
          ]
        },
        response: {
          delayMs: 0,
          statusCode: '200',
          headers: {
            'content-type': 'application/json'
          },
          body: '{"orderId":"mock-001","status":"PAID"}'
        }
      }
    ]
  }
}
```

映射规则：

| 页面字段 | 预览字段 |
|---|---|
| Mock 服务命名空间 | `spec.scope.namespace` |
| Mock 服务名称 | `spec.scope.service` |
| 子规则序号 | `spec.rules[].name = mock-subrule-{n}` |
| 协议 | `spec.rules[].interface.protocol` |
| 方法 | `spec.rules[].interface.method` |
| 接口路径 | `spec.rules[].interface.path` |
| 路径匹配 | `spec.rules[].interface.op` |
| 匹配关系 | `spec.rules[].match.relation` |
| 匹配条件 | `spec.rules[].match.conditions[]` |
| 响应延迟 | `spec.rules[].response.delayMs` |
| 响应 Code | `spec.rules[].response.statusCode` |
| Header Key-Value | `spec.rules[].response.headers` |
| Body | `spec.rules[].response.body` |

不要输出：

- `ratioPercent`
- `reason`
- 固定的 `contentType` 主字段

---

## 9. 保存校验

通用校验：

- 规则名称必须符合当前规则编辑器的命名要求。
- 至少配置 1 条 Mock 子规则。

每条 Mock 子规则校验：

| 校验项 | 规则 | 错误文案建议 |
|---|---|---|
| 接口路径 | 不能为空 | `Mock 子规则[n] 接口路径不能为空` |
| 返回延迟 | 必须 >= 0 | `Mock 子规则[n] 返回延迟不能小于 0` |
| 响应 Code | 字符串非空 | `Mock 子规则[n] 响应 Code 不能为空` |
| 流量匹配条件 | 至少 1 条 | `Mock 子规则[n] 请至少配置 1 个流量匹配条件` |
| 匹配值 | 不能为空 | `Mock 子规则[n] 流量匹配条件存在空匹配值` |
| Header 名 | Header 值非空时 Header 名不能为空 | `Mock 子规则[n] 响应头存在空 Header 名` |
| JSON Body | 普通 Header 中 `content-type` 包含 `json`，且 Body 非空时必须是合法 JSON | `Mock 子规则[n] Content-Type 为 JSON 时，响应正文必须是合法 JSON` |

不要校验：

- Mock 比例范围。
- 响应 Code 的 `100–599` 数字范围。
- Reason。
- 固定 Content-Type 主字段。

---

## 10. 关键交互状态

### 10.1 添加 Mock 子规则

点击 `添加 Mock 子规则` 后新增一条规则，默认值建议：

```js
{
  open: true,
  iface: {
    proto: 'HTTP',
    method: 'GET',
    path: '',
    op: '完全匹配'
  },
  relation: 'AND',
  conditions: [
    { type: 'HEADER', key: 'x-mock', match: '完全匹配', value: '' }
  ],
  delay: 0,
  code: '200',
  headers: [
    { k: 'content-type', v: 'application/json' }
  ],
  body: '{}'
}
```

### 10.2 删除 Mock 子规则

- 多于 1 条时显示删除按钮。
- 删除后重新渲染列表和右侧预览。
- 如果实现允许删到 0 条，保存时必须拦截；当前交互建议至少保留 1 条。

### 10.3 修改 Body

普通 textarea 输入时：

1. 更新当前子规则 `body`。
2. 重新计算 textarea 高度。
3. 刷新最终返回预览。
4. 刷新右侧预览映射。

全屏编辑输入时：

1. 更新同一个 `body` 字段。
2. 同步普通 textarea 的值。
3. 同步最终返回预览。
4. 同步右侧预览。

### 10.4 Body 高度算法

目标：

- 1 行内容 → 1 行高度。
- 3 行内容 → 3 行高度。
- 15 行以内 → 按当前内容高度。
- 超过 15 行 → 固定 15 行高度，内部滚动。

伪代码：

```js
function fitMockBodyEditor(textarea) {
  textarea.classList.remove('is-scroll');
  textarea.style.height = 'auto';

  const line = getLineHeight(textarea);
  const padding = getVerticalPadding(textarea);
  const min = line + padding + 2;
  const max = line * 15 + padding + 2;
  const next = Math.min(Math.max(textarea.scrollHeight, min), max);

  textarea.style.height = `${next}px`;
  textarea.classList.toggle('is-scroll', textarea.scrollHeight > max + 1);
}
```

实现注意：

- 普通 Body textarea 不要有固定大 `min-height`。
- 不要让 `.mock-body-wide textarea` 这类宽编辑区样式把普通 textarea 撑到大文本框。
- 全屏编辑器可以有较大高度；普通态不能继承全屏高度。

---

## 11. 视觉与布局要求

Mock 响应结果：

- 统一使用一个响应面板。
- 内部用分区标题和细分隔线表达层级。
- 不要卡片套卡片。
- 不要在 Code/Header/Body 每层都套白底、边框、阴影。
- Code 与延迟左右并排。
- Header 与 Body 上下层级。
- Body 占满宽度。
- 最终返回预览使用深色代码块样式即可。

推荐层级：

```text
Mock 响应结果
├── 响应 Code / 延迟
│   ├── 响应 Code
│   ├── 响应延迟
│   └── 执行顺序提示
├── 响应 Header
├── 响应 Body
└── 最终返回预览
```

不要这样做：

```text
Mock 响应结果
├── 响应状态卡
├── 延迟卡
├── Header 卡
├── Body 卡
└── 预览卡
```

---

## 12. 反回归清单

后续实现不要把以下内容加回来：

1. 不要把 Mock 做成单条全局接口规则。
2. 不要让用户配置 Mock 比例、流量百分比或 `ratioPercent`。
3. 不要在响应 Code 旁边展示 Reason。
4. 不要把响应 Code 强制成 HTTP 数字状态码。
5. 不要用 `100–599` 校验响应 Code。
6. 不要把响应 Header 绑定成固定 `Content-Type` 主控件。
7. 不要把 Header 和 Body 做成左右并列，Body 会被挤窄。
8. 不要让 Body 普通态固定成大文本框。
9. 不要把执行顺序提示放进最终返回预览主体。
10. 不要让 Mock 响应结果内部出现多层白底卡片堆叠。
11. 不要把本文档描述为后端 schema、底层数据结构或协议契约调整。

---

## 13. 实现清单

页面实现需要覆盖：

- [ ] Mock 服务信息在大规则层展示。
- [ ] Mock 子规则支持多条添加 / 删除。
- [ ] 每条子规则包含 `选择接口`、`流量匹配`、`Mock 响应结果` 三段。
- [ ] 子规则不展示 Mock 比例。
- [ ] 响应 Code 支持字符串。
- [ ] 响应延迟与响应 Code 左右并排。
- [ ] 执行顺序提示位于 Code / 延迟行下方。
- [ ] 响应 Header 是普通 Key-Value 列表。
- [ ] 响应 Body 占满宽度。
- [ ] Body 未满 15 行时按内容高度自适应。
- [ ] Body 超过 15 行后内部滚动。
- [ ] Body 提供全屏编辑入口。
- [ ] 最终返回预览展示 Code、Header、Body。
- [ ] 右侧预览映射输出 `rules[]`。
- [ ] 右侧预览不输出 `ratioPercent`、`reason`。
- [ ] 保存校验覆盖接口路径、延迟、Code、匹配条件、Header 名、JSON Body。

---

## 14. 建议验证

实现后至少做以下检查：

1. 打开 Mock 规则，默认渲染不少于 1 条子规则。
2. 添加第二条 Mock 子规则后，右侧预览出现 2 条 `rules[]`。
3. 右侧预览不包含 `ratioPercent`。
4. 右侧预览不包含 `reason`。
5. 将响应 Code 改为 `MOCK_OK`，保存校验通过，预览保留该字符串。
6. 清空响应 Code，保存校验提示 `响应 Code 不能为空`。
7. 清空接口路径，保存校验提示接口路径不能为空。
8. 清空匹配值，保存校验提示流量匹配条件存在空匹配值。
9. Header 值非空但 Header 名为空时，保存校验拦截。
10. `content-type=application/json` 且 Body 非法 JSON 时，保存校验拦截。
11. 普通 Body 1 行内容时高度贴近 1 行。
12. 普通 Body 3 行内容时高度贴近 3 行。
13. 普通 Body 超过 15 行时固定上限并内部滚动。
14. 全屏编辑 Body 后，普通 textarea、最终返回预览、右侧预览同步更新。

## 相关页面
