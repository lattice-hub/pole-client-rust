---
title: 泳道规则编辑 — 设计交接文档
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# 泳道规则编辑 — 设计交接文档

> 适用范围：Pole.IO 治理工作台 → 规则类型 = **泳道（LaneGroup / LaneRule）** 编辑抽屉。
> 视觉体系沿用路由规则同一套 Ant 风格令牌（浅灰底 `#f0f2f5` + 白卡片 + 主色 `#1677ff`），原型见 `index.html`。
> 与路由规则共享的分段样式、控件、悬浮提示规范见 `router-rule-design.md`，本文只描述泳道特有部分。

---

## 1. 整体结构 —— 泳道组与泳道分开编辑

泳道编辑器按**页面状态（Tab）**拆分，泳道组与泳道规则不在同一个编辑页混合编辑：

```
泳道规则（lane）  —— 泳道定义列表，每条 = 一条泳道规则；只选择已存在泳道组
泳道组（group）   —— 泳道组入口 + 组内服务集合
版本（version）   —— 历史版本快照（只读）
审计（audit）     —— 操作记录（只读）
```

设计约束：

- **泳道组页**负责「组服务 / 入口」；**泳道页**只选择已存在的泳道组并编辑泳道规则，两者不混在同一个编辑页。
- 版本、审计只读，不在这两个页里编辑配置，避免把运维记录混入编辑表单。
- 右栏实时 Spec 随页面切换不同 `kind`（`LaneGroup` / `LaneRule` / `LaneGroupVersion` / `LaneGroupAudit`）。

---

## 2. 泳道组页（group）

### 2.1 泳道组入口

配置哪些网关或应用入口进入该泳道组判定。列表 + 抽屉草稿新增。

| 列 | 控件 | 取值 |
|---|---|---|
| # | 序号 | 自增 |
| 入口类型 | **固定展示**（已选后只读） | `网关入口` / `应用入口` |
| 命名空间 | 下拉 | 治理命名空间 |
| 服务 | 下拉 | 命名空间下的服务 |

交互：

- 新增走草稿表单：选入口类型（分段）→ 命名空间 → 服务 → 「添加泳道组入口」。
- **已创建的入口类型作为固定信息展示，不再提供可切换下拉。**
- 至少保留 1 个入口，入口数大于 1 时才显示删除按钮。

### 2.2 组内服务

维护泳道组服务集合，已加入服务只允许删除（不可改名）。

- 列表展示服务名 + 被引用次数（被多少条泳道引用）。
- 新增走草稿：从可选服务里挑一个加入。
- 泳道规则里的「组内服务」下拉只能从这个集合里选。

---

## 3. 泳道规则页（lane）

### 3.1 泳道定义列表

标题「泳道定义」，每条列表项对应泳道组内的一条泳道规则。底部「新建泳道」；若泳道组尚无服务则禁用并提示「需先创建泳道组服务」。

每条泳道是一张可折叠卡片：

- **折叠态**：泳道名 + 摘要 + 启用开关 + 删除按钮。
- **摘要文本**：`{匹配规则数} 个匹配规则 · 放量 {N}% · {组内服务数} 个组内服务 · X-Lattice-Traffic-Lane={泳道标签}`。
- **展开态**：泳道名 / 泳道标签 Value → 固定标签行 → ① 流量匹配规则 → ② 组内服务 → ③ 流量拓扑演示。

### 3.2 泳道标识

| 字段 | 控件 | 说明 |
|---|---|---|
| 泳道名称 | 输入框 | 如 `shadow-lane` |
| 泳道标签 Value | 输入框 | 如 `shadow` / `stable` |
| 固定标签 Key | 只读 | `X-Lattice-Traffic-Lane`（常量，全局固定） |

固定标签行展示 `X-Lattice-Traffic-Lane = {泳道标签 Value}`，命中后写入该标签把流量导向对应泳道。

### 3.3 ① 流量匹配规则（含放量比例）

命中后进入该泳道的匹配条件，支持 AND/OR 与多行增删；右侧新增**命中后放量比例**。

| 字段 | 控件 | 取值 |
|---|---|---|
| 关系 | 分段按钮 | `AND`（需同时满足全部规则）/ `OR`（满足任一即可） |
| 参数类型 | 下拉 | `HEADER` / `QUERY` / `PATH` / `COOKIE` / `METHOD` |
| 参数键 | 输入框（mono） | 如 `x-shadow`、`x-lane` |
| 匹配类型 | 下拉 | `完全匹配` / `前缀匹配` / `正则匹配` / `包含` / `不等于` |
| 匹配值 | 输入框 | 如 `true`、`stable` |
| **命中后放量** | 数字输入 + `%` 后缀 | **0–100，默认 100** |

放量比例语义：

- 命中匹配条件的流量中，按 **N%** 放量进入本泳道，其余 **(100−N)%** 回落基线版本。
- **100% 即全量进入**；用于灰度放量场景（如先放 30% 验证 shadow 泳道）。
- 输入时实时刷新 Spec 与拓扑；失焦夹取到 0–100。
- 条下提示：`命中匹配条件的流量中，按 N% 放量进入本泳道，其余回落基线版本（100% 即全量进入）。`

### 3.4 ② 进入泳道的组内服务

只能选择当前泳道组已有服务。每行 = 服务下拉 + 固定标签展示 `X-Lattice-Traffic-Lane={泳道标签}`。至少 1 个服务，服务数大于 1 时显示删除。

底部说明：`命中 {泳道名} 后，所选组内服务会按固定标签 X-Lattice-Traffic-Lane={泳道标签} 进入对应泳道。`

### 3.5 ③ 流量拓扑演示

展示本条泳道规则命中后的流量路径，SVG 服务网格图谱风格（类 Kiali / Istio），仅用绿 / 灰两色：

```
泳道组入口 → 流量匹配 ─┬─(绿 实线·放量 N%)→ 命中泳道 [作用服务集群]
                       └─(灰 虚线·回落 100−N%)→ 基线版本
```

- **命中放量分支**（绿色实线）：从匹配节点引向泳道三角，线上标 `放量 N%`，扇出到作用服务。
- **回落分支**（灰色虚线）：从匹配节点同一分叉点引向基线节点，线上标 `回落 (100−N)%`。
- N = 100 时：绿线 `放量 100%`，灰虚线退化为 `未命中`，基线节点写「未命中 → 基线版本」。
- **作用服务示意**：≤ 3 个全展示；> 3 个只示意「前两个 + `⋯ 省略 N 个` + 最后一个」，分组头标注「共 N 个服务」。
- 文字加白色描边（`paint-order:stroke`）压在连线上仍清晰；窄屏自适应缩放。
- 图例：`命中放量进泳道 / 回落·未命中 → 基线 / 本规则选择的服务`。

---

## 4. 前端数据模型（M 结构）

```js
lane = {
  name: 'spec-check-lane-group',
  enabled: true,
  priority: 5,
  desc,
  tags: [],
  lanePage: 'lane',                       // group | lane | version | audit
  laneGroup: { name: 'spec-check-lane-group', status: 'CREATED' },
  entries: [
    { kind: 'gateway', ns: 'spec-governance', svc: 'spec-gateway' },
    { kind: 'app',     ns: 'spec-governance', svc: 'spec-order' }
  ],
  selected: ['spec-order (spec-governance)', 'spec-payment (spec-governance)'],
  lanes: [
    {
      name: 'shadow-lane',
      on: true,
      open: false,
      laneValue: 'shadow',                // X-Lattice-Traffic-Lane 的取值
      relation: 'AND',                    // AND | OR
      matchRatio: 30,                     // 命中后放量比例 0–100
      conditions: [
        { type: 'HEADER', key: 'x-shadow', match: '完全匹配', value: 'true' }
      ],
      serviceTags: [
        { service: 'spec-order' },
        { service: 'spec-payment' }
      ]
    }
  ]
}
```

常量：`LANE_TRAFFIC_TAG_KEY = 'X-Lattice-Traffic-Lane'`（固定标签 Key，不可编辑）。

归一化要求：

- `matchRatio` 缺省补 `100`，并夹取到 0–100。
- `relation` 缺省 `AND`；`conditions` / `serviceTags` 缺省补一条。
- 组内服务超出泳道组集合时回落到集合首个服务。

---

## 5. 实时 Spec

### 5.1 LaneRule（泳道规则页，YAML 示例）

```yaml
apiVersion: governance.pole.io/v1
kind: LaneRule
metadata:
  name: spec-check-lane-group
  enabled: true
  priority: 5
spec:
  description: spec LaneGroup + LaneRule sample
  laneGroup:
    name: spec-check-lane-group
    status: CREATED
  lanes:
    - name: shadow-lane
      enabled: true
      note: shadow 验证流量进入灰度泳道
      match:
        relation: AND
        ratioPercent: 30
        conditions:
          - { param: HEADER, key: x-shadow, op: 完全匹配, value: "true" }
      services: [spec-order, spec-payment]
      laneTag: { key: X-Lattice-Traffic-Lane, value: shadow }
```

### 5.2 LaneGroup（泳道组页，YAML 示例）

```yaml
apiVersion: governance.pole.io/v1
kind: LaneGroup
metadata:
  name: spec-check-lane-group
spec:
  description: spec LaneGroup + LaneRule sample
  laneGroup:
    name: spec-check-lane-group
    status: CREATED
    entries:
      - { type: micro-gateway, namespace: spec-governance, service: spec-gateway }
      - { type: micro-app,     namespace: spec-governance, service: spec-order }
    services: [spec-order, spec-payment]
```

版本 / 审计页分别输出 `LaneGroupVersion` / `LaneGroupAudit`（只读快照与事件列表）。

字段映射：

| Spec 字段 | 来源 |
|---|---|
| `metadata` | 基础信息 |
| `spec.laneGroup.entries[]` | 泳道组页.入口 |
| `spec.laneGroup.services[]` | 泳道组页.组内服务 |
| `spec.lanes[].match.relation` | 泳道.① 关系 |
| `spec.lanes[].match.ratioPercent` | 泳道.① 命中后放量 |
| `spec.lanes[].match.conditions[]` | 泳道.① 匹配条件 |
| `spec.lanes[].services[]` | 泳道.② 组内服务 |
| `spec.lanes[].laneTag` | 固定标签 Key + 泳道标签 Value |

---

## 6. 校验规则（保存时）

保存前全量校验，任一不通过则拦截、Toast 首条错误并高亮字段。

**泳道组页：**

1. 泳道组名称不能为空。
2. 至少配置 1 个泳道组入口，且入口命名空间 / 服务完整。
3. 至少纳入 1 个泳道组服务。

**泳道规则页：**

1. 至少配置 1 条泳道，且不存在未命名泳道。
2. 每条泳道：
   - **命中后放量比例必须在 0–100 之间；**
   - 至少 1 个引流条件，且不存在空条件；
   - 泳道标签 Value 不能为空；
   - 至少 1 个组内服务，不存在空服务；
   - 不引用泳道组之外的服务。

校验通过 → 持久化（原型用 localStorage）→ Toast「已保存并下发到数据面」→ 关闭抽屉。

---

## 7. 视觉与布局要点

- 延续 Ant Design 企业控制台风格：浅灰背景、白卡片、发丝边框、蓝色主操作。
- 泳道组与泳道**分页编辑**，不混在同一编辑页。
- 入口类型已选后**固定展示**，不做可切换下拉。
- ① 流量匹配规则的「命中后放量」与 AND/OR 关系条同行，比例输入右对齐 + `%` 后缀。
- ③ 流量拓扑只用绿 / 灰两色，单一主调；多服务时折叠示意，不堆砌节点。
- 折叠摘要优先展示可判断行为的核心信息：匹配规则数、放量比例、服务数、泳道标签。
- Spec 右栏保持深色代码面板，YAML / JSON 切换和复制与其它规则一致。

---

## 8. 实现要点清单

- [ ] 泳道组 / 泳道 / 版本 / 审计四个页面状态拆分，互不混编。
- [ ] 泳道组页：入口列表（类型只读）+ 组内服务集合（只允许删除已加入服务）。
- [ ] 泳道规则页：泳道卡片增删 + 折叠，摘要随编辑实时刷新。
- [ ] ① 匹配规则支持 AND/OR + 多行增删；新增「命中后放量」0–100 输入。
- [ ] ② 组内服务下拉只能选泳道组已有服务，带固定标签展示。
- [ ] ③ 流量拓扑：命中放量 / 回落双分支按比例标注；多服务折叠示意。
- [ ] 右栏实时 Spec 按页面切换 kind，输出 `match.ratioPercent` 等字段。
- [ ] 保存校验覆盖泳道组入口 / 服务、放量比例、匹配条件、泳道标签、组内服务。

## 相关页面
