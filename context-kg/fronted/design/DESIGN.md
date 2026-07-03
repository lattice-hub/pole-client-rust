---
title: Pole.IO 治理规则 · 设计系统（DESIGN.md）
tags: [fronted, design, governance]
links: []
updated: 2026-07-04
sources: 1
---

# Pole.IO 治理规则 · 设计系统（DESIGN.md）

治理规则编辑控制台的视觉与交互规范，从 `index.html` 实现中提炼。基于 **Ant Design 5 体系**的企业级控制台：浅灰底、白卡片、单一蓝主色、右侧编辑抽屉 + 深色 Spec 实时预览。供后续所有治理规则相关界面延续使用。

---

## 1. 设计基调

- **角色**：系统设计师 / 控制台设计师 —— 信息密度即功能，不做营销式视觉。
- **气质**：冷静、技术、工具化。发丝线 + 留白承担分隔，几乎无投影。
- **主色预算**：每屏蓝色只出现在「主操作 + 开关 / 编号徽标」两处，不泛滥。
- **数值即一等公民**：所有数字、ID、规则 spec 一律 mono 字体 + `tabular-nums`。

---

## 2. 设计 Token（`:root`）

### 色彩

| Token | 值 | 用途 |
|---|---|---|
| `--bg` | `#f0f2f5` | 页面底 |
| `--surface` | `#fff` | 卡片 / 抽屉 / 输入 |
| `--raised` | `#fafbfc` | 内嵌区 / 表头 / 只读标签 |
| `--fg` | `rgba(0,0,0,.88)` | 主文字 |
| `--fg-2` | `rgba(0,0,0,.65)` | 次级文字 / label |
| `--muted` | `rgba(0,0,0,.45)` | 说明 / 提示 |
| `--faint` | `rgba(0,0,0,.25)` | 占位 / 图标默认 |
| `--border` | `#ebedf0` | 发丝分隔线 |
| `--border-2` | `#e3e6eb` | 输入 / 控件描边 |
| `--accent` | `#1677ff` | 主色：按钮 / 开关 / 链接 / 编号徽标 |
| `--accent-hover` | `#4096ff` | hover |
| `--accent-active` | `#0958d9` | active / 强调文字 |
| `--accent-wash` | `#e8f1ff` | 主色浅底：选中态 / hover 背景 |
| `--success` | `#52c41a` | 启用 / 放通（wash `#f0f9eb`）|
| `--warning` | `#faad14` | 警告（wash `#fffbe6`）|
| `--danger` | `#ff4d4f` | 拒绝 / 删除 / 校验失败（wash `#fff1f0`）|

### 字体

```css
--font: -apple-system, BlinkMacSystemFont, "PingFang SC", "Microsoft YaHei", system-ui, sans-serif;
--mono: "SF Mono", "JetBrains Mono", ui-monospace, Menlo, monospace;
```

- 正文 / 标题用 `--font`，14px 基准、`line-height:1.5715`。
- **mono 用于**：数值、服务名 / 命名空间、接口路径、Header、标签 chip、规则编号徽标、Spec 预览、textarea 报文。

### 形状与高度

| Token | 值 |
|---|---|
| `--radius` | `8px`（卡片 / panel / 抽屉区块）|
| `--radius-sm` | `6px`（按钮 / 输入 / 控件）|
| 圆形 | 编号徽标、状态点、头像 |
| 控件高度 | 按钮 32px、输入 34px（紧凑表内 30/32px）|
| `--shadow-drawer` | `-8px 0 32px rgba(0,0,0,.12)` |
| `--shadow-pop` | `0 6px 16px rgba(0,0,0,.08), 0 3px 6px rgba(0,0,0,.06)` |

> 投影**仅**用于抽屉、下拉菜单、popover。卡片、表格、表单区块一律无投影，靠 1px 发丝线分隔。

---

## 3. 规则类型语义配色（kind-tag）

九种治理规则各有固定标签色，全控制台一致。标签样式：`1px 8px / radius 4px / 12px 500 字重 / 文字色 + wash 底 + 描边色`。

| 规则 | 类名 | 文字 | 底 | 描边 |
|---|---|---|---|---|
| 路由 | `.kind-route` | `#1677ff` | `#e8f1ff` | `#bcd9ff` |
| 泳道 | `.kind-lane` | `#722ed1` | `#f4ecff` | `#d9c4f5` |
| Mock | `.kind-mock` | `#fa8c16` | `#fff3e0` | `#ffd591` |
| 鉴权 | `.kind-auth` | `#13c2c2` | `#e6fffb` | `#9ae8e3` |
| 故障探测 | `.kind-fault` | `#fa541c` | `#fff2e8` | `#ffbb96` |
| 无损上下线 | `.kind-lossless` | `#389e0d` | `#f6ffed` | `#b7eb8f` |
| 镜像 | `.kind-mirror` | `#eb2f96` | `#fff0f6` | `#ffadd2` |
| 限流 | `.kind-ratelimit` | `#2f54eb` | `#f0f5ff` | `#adc6ff` |
| 熔断 | `.kind-circuit` | `#d48806` | `#fffbe6` | `#ffe58f` |

> 这套色是规则身份系统：列表标签、抽屉头、拓扑高亮都用同一规则的色，让用户一眼区分规则类型。

---

## 4. 整体布局

```
┌─────────┬────────────────────────────────────┐
│ 侧边导航 │ 顶栏（面包屑 + 用户）                  │
│ 208px   ├────────────────────────────────────┤
│         │ 工作区：标题 + 统计行 + 规则面板/表格    │
└─────────┴────────────────────────────────────┘
        点击规则行 → 右侧抽屉滑入（min(1180px, 92vw)）
```

- **侧边导航** `208px`：底 `#f7f8fa`，分组标题 + 缩进导航项，选中态 = 蓝字 + `accent-wash` 底 + 左侧 3px 蓝条。
- **顶栏 / 抽屉头** 高 50–54px，白底发丝线。
- **工作区** `padding:22px 28px`，整页 `overflow:hidden`、内部各区独立滚动。
- 响应式：`≤1100px` 侧栏收成 64px 图标条、抽屉 Spec 预览隐藏。

---

## 5. 核心组件规范

### 统计卡 `.stat`
白卡发丝线，`k` 说明（muted 12.5px）+ `v` 数值（**mono 26px 600 tabular-nums**）+ `d` 增量（faint）。

### 规则列表 `table.rules`
- 表头 `--raised` 底、muted 500 12.5px；单元格 13–11px padding 发丝线分隔。
- 行 hover `#f7faff`；规则名 `.rname` 用蓝色可点击。
- 操作列 `.row-act` 蓝色文字链接（hover 下划线），删除类用 danger。
- 状态点 `.dot`：绿点 = 启用，灰点 = 停用。
- 筛选 chip `.kchip`：药丸描边，选中填蓝，带 mono 计数。

### 编辑抽屉 `.drawer`
从右滑入（`cubic-bezier(.4,0,.2,1) .28s`），结构：
1. **抽屉��** `.dr-head`：规则名（mono 15px 600）+ kind-tag + 关闭。
2. **页签** `.dr-tab`：底部 2px 蓝条标记 active。
3. **双栏 body** `.dr-body`：`1fr 380px` —— 左侧表单滚动区，右侧深色 Spec 预览。

**抽屉宽度与滚动约束(所有规则类型一致)：**
- 全部规则共用同一抽屉 `#drawer`,宽度固定 `min(1180px, 92vw)`,不得按规则类型改宽度。
- 表单区**只允许纵向滚动**:`.dr-form` 设 `overflow-x:hidden` + `container-type:inline-size`,任何规则信息都不能要左右滚动才看清。
- 密集多列编辑器(Mock 响应、镜像子规则、泳道行、调用关系、鉴权范围等)按**表单真实宽度**用 `@container drawerForm` 收起,而非按视口:`>820px` 才两栏(如 Mock `minmax(0,.82fr) minmax(0,1.18fr)`),`≤680px` 收为单列/紧凑,`≤520px` 进一步精简。固定 `px` 最小列宽之和不得超过表单内容区(满宽约 716px),否则改用 `minmax(0,…)` 或默认堆叠。

### 编号分段 `.sec`
白卡，段头 `.sec-h` = 蓝色圆形编号徽标 `.no`（22px mono）+ 标题 + 可选 hint。这是规则配置的主组织单元。

### 表单控件
- `.field` + `.ctl`：label（fg-2 13px）在上，34px 输入框；focus `accent` 描边 + 2px wash 光环。
- 必填星号 `.req` danger 色；校验失败 `.field.invalid` 红边 + `.err-msg`。
- 数字步进器 `.stepper`、开关 `.sw`（22px 高，开启变蓝右移）、标签 `.tag`（accent-wash 底 + mono）、单选 `.radio`（选中蓝边 wash 底）。
- 穿梭框 `.xfer`：`1fr 46px 1fr` 双列选择，用于泳道组等多选场景。

### 权重条（路由）`.wbar` / `.wsum`
分段彩条（`c0`蓝/`c1`绿/`c2`橙/`c3`紫/`c4`青）+ 合计实时校验：等于 100% 显示 `.wpill.ok` 绿胶囊，否则 `.bad` 红胶囊。

### 流量拓扑 `.lane-topo`
- 内联 **SVG 开放画布**（非图片），径向浅蓝渐变底、无卡片重背景。
- 统一表达 `client → 控制面 → 上游`，不同规则**高亮被改写的流量路径**（实线绿 = 命中，灰虚线 = 旁路）。
- 配图例 `.lane-topo-legend`。交互式，可点小圆点切换，不用数字索引。

### Spec 实时预览 `.spec`
- **深色面板**（GitHub Dark）：底 `#0d1117`、文字 `#c9d1d9`、左发丝边 `#1f2630`。
- 头部 uppercase 标题 + 格式 seg 切换（YAML/JSON，选中 `#1f6feb`）+ 复制按钮。
- `.spec-pre` mono 12.5px、`line-height:1.7`、`white-space:pre`。
- 脚 `.spec-foot`：校验通过 `.ok` 绿 `#3fb950`，失败 `.bad` 红 `#f85149`。
- 任一输入 `oninput` 都触发 `renderSpec()`，schema 按规则类型切换（RouteRule/LaneGroup/MockRule/AuthRule…）。

---

## 6. 按钮层级

| 类 | 样式 |
|---|---|
| `.btn.primary` | 蓝底白字，主操作（保存 / 创建）|
| `.btn` | 白底描边，hover 变蓝边蓝字，次操作 |
| `.btn.ghost` | 透明，hover 浅灰底，弱操作 |
| `.mini` | 34px 方形图标按钮，删除 hover 变红；`.mini.primary` 蓝底 |
| `.add-row` / `.add-block` | 蓝色文字 / 虚线框「+ 添加」 |

---

## 7. 规则编辑的结构原则（交互契约）

这些是已确认、需延续的规则建模约定：

- **抽屉第②段按规则类型分发**，不同规则有视觉上可区分的内部编辑结构，绝不复用同一套「匹配条件 + 目标分组」：
  - **路由**：AND/OR 匹配条件 + 目标分组权重（实时 =100% 校验）。
  - **泳道**：泳道入口（网关 / 应用，已选类型只读展示）+ 泳道组穿梭选择器 + 每泳道匹配条件；泳道组与泳道**拆成独立编辑入口**，不混在同一页。
  - **Mock**：接口范围 + Mock 响应（Code 与延迟同行并排、Header/Body 上下层级，Body ≤15 行自适应、超出内部滚动 + 全屏入口，无 reason）。
  - **鉴权**：外层先分黑 / 白名单（子规则内不再重复名单类型）；服务信息作为独立板块置于第①②部分之间；匹配策略只保留 AND/OR + 匹配条件，不展示策略名 / 比例。
  - **镜像**：服务级规则下挂多条子规则（接口 + 流量标签 + 镜像比例 + 目标服务），无持续时间，镜像目标只在子规则内。
  - **探测**：被探测对象下挂多条独立探测规则（协议 / 启停 / 间隔 / 超时 / 端口 / 方法 / URL / Headers），TCP/UDP 支持报文匹配、端口默认自动匹配。
  - **无损上下线**：上线只含延迟注册策略 + 探测接口，无触发阶段。
- **已选 / 已创建资源的类型字段**按只读信息展示，不再做可切换下拉。
- 优化属于**前端展示映射与组织方式调整**，不宣称底层数据契约变更。

---

## 8. 反「AI slop」守则（治理控制台版）

- ❌ 渐变背景洪流、emoji 图标、左边框彩条卡片、暖米 / 桃 / 粉底。
- ❌ 把规则类型做成「图形隐喻各异」的卡 —— 七 / 九张规则卡用**同一张流量拓扑**，仅高亮不同改写路径。
- ❌ 在产品 UI 里暴露设计元数据、视口切换、平台开关、demo 控件。
- ✅ 发丝线分隔、tabular-nums 数值、mono 承载技术信息、单蓝主色克制使用、深色 Spec 预览作为唯一的「技术质感」落点。

---

## 9. 控件清单（实现规范）

每个控件的尺寸、圆角、配色 token 与全套交互状态。可直接作为前端实现依据。

### 9.1 输入 / 下拉 `.ctl`

| 属性 | 值 |
|---|---|
| 高度 | `34px`（紧凑表内 `30/32px`）|
| 内边距 | `0 11px`（textarea `8 11px`）|
| 圆角 / 边框 | `--radius-sm 6px` / `1px --border-2` |
| 底色 | `#fff` |

| 状态 | 表现 |
|---|---|
| default | `1px --border-2` 描边 |
| focus-within | 边框 `--accent` + 外发光 `0 0 0 2px --accent-wash` |
| invalid | 边框 `--danger`，下方 `.err-msg` 12px danger 显示 |
| select | 同 `.ctl`，右侧 `.chev` 12px faint 箭头，`appearance:none` |
| textarea | `min-height:60px`、`resize:vertical`、mono 12.5px |

### 9.2 按钮（四档层级）

| 档 | 类名 | 尺寸 | default | hover |
|---|---|---|---|---|
| 主操作 | `.btn.primary` | 32px / `0 15px` / 13.5px | accent 底白字 | `--accent-hover` 底 |
| 次操作 | `.btn` | 同上 | 白底 + border-2 描边 | accent 字 + accent 边 |
| 弱操作 | `.btn.ghost` | 同上 | 透明 | `#f0f2f5` 底 |
| 图标方钮 | `.mini` | `34×32`（行内 `width:100%`）| border-2 + faint 图标 | 边/图标转 danger；`.mini.primary` = accent 底白字 |

图标 `14px`,圆角统一 `--radius-sm`。

### 9.3 开关 `.sw`

| 变体 | 轨道 | 滑块 | 开启位移 |
|---|---|---|---|
| 标准 `.sw` | `40×22` / `radius 11` | `18px` 白圆 + `0 1px 3px` 投影 | `left 2→20` |
| 小号 `.sw.sm` | `32×18` | `14px` | `left 2→16` |

关闭 `#bfbfbf`,开启 `--accent`;过渡 `.18s`。配 `.sw-row .lab`(fg-2 13px)。

### 9.4 数字步进器 `.stepper`

`120×34`,border-2 + radius-sm。两侧 `-/+` 按钮 `34px` 宽、`--raised` 底,hover 转 accent + `#eef4ff`;中间 input mono + `tabular-nums` + 居中。

### 9.5 单选 `.radio`

`padding 8 14`,border-2 + radius-sm。圆点 `.rd` 15px、`1.5px --faint` 边。

| 状态 | 表现 |
|---|---|
| default | border-2 描边、faint 圆环 |
| on | 边框 + 文字转 `--accent-active`、底 `--accent-wash`、圆环内 `7px` accent 实心点 |

### 9.6 分段选择器 `.seg-pick` / `.rel-seg`

外框 border-2 + radius-sm + `overflow:hidden`,按钮 `padding 5–6px 14px` / 12.5px。

| 变体 | 字体 | 用途 |
|---|---|---|
| `.seg-pick` | `--font` 常规 | 范围 / 选项切换 |
| `.rel-seg` | `--mono` 600 | AND / OR 关系切换 |

选中态 `.on` = `--accent` 底白字;旁配 `.rel-note`(muted 12px)说明语义。

### 9.7 标签 / 筛选 chip

| 控件 | 类名 | 尺寸 | 配色 |
|---|---|---|---|
| 值标签 | `.tag` | h26 / `0 9px` / radius-sm | accent-wash 底 + `--accent-active` 字 + `#bcd9ff` 边 + mono;`.rm` 关闭 opacity .6→1 |
| 添加标签 | `.tag-add` | h26 | 虚线 border-2 + muted,hover 转 accent |
| 筛选 chip | `.kchip` | h30 / `0 13px` / radius 999 | 描边药丸 + mono `.cnt` 计数;hover accent;`.on` = accent 底白字 |

### 9.8 状态指示

| 控件 | 类名 | 表现 |
|---|---|---|
| 状态点 | `.dot` / `.dot.off` | `6px` 圆点,启用 `--success` / 停用 `--faint` + muted 字 |
| 校验胶囊 | `.wpill.ok` / `.bad` | `radius 20` 小胶囊;ok = success-wash 底 `#389e0d` 字 / bad = danger-wash 底 `#cf1322` 字 |
| 规则标签 | `.kind-tag` ×9 | 见 §3,文字 + wash 底 + 描边三色一组 |

### 9.9 添加入口

| 控件 | 类名 | 表现 |
|---|---|---|
| 行内添加 | `.add-row` | accent 13px 文字「+ 添加」,hover `--accent-active` |
| 整块添加 | `.add-block` | 虚线 border-2 + radius-sm,`width:100%`、`height:38px` 居中 |

### 9.10 复合行布局（规则专属,非通用原语）

以下是各规则类型用 CSS Grid 拼出的「字段行」,随 schema 而定,不作为复用控件:

| 类名 | 规则 | 列结构 |
|---|---|---|
| `.iface-row` | 通用接口行 | 协议 / 方法 / 路径 / 备注 / 删除 |
| `.kv-row` | Header 键值 | 键 / 值 / 删除 |
| `.win-row` | 限流窗口 | 阈值 / 窗口 / 删除 |
| `.cb-irow` / `.cb-erow` / `.cb-trow` | 熔断 | 接口 / 错误 / 阈值多列 |
| `.lane-entry-row` / `.lane-group-row` | 泳道 | 序号 / 类型 / 入口 / 值 |
| `.auth-scope` / `.auth-path` / `.auth-decision` | 鉴权 | 服务范围 / 路径 / 决策双列 |
| `.callgrid` + `.node.src/.dst` | 调用关系 | 主调 → 被调双节点 |

这些行在 `≤1100px` / `≤760px` 有对应的 grid 重排(详见各规则 `*-rule-design.md`)。

---

## 10. 控件代码片段（可复制）

CSS 取自 `index.html` 实现,依赖 §2 的 `:root` token。每段含样式 + 最小 HTML 用法。

### 10.1 输入 / 下拉 `.ctl`

```css
.ctl{height:34px;border:1px solid var(--border-2);border-radius:var(--radius-sm);background:#fff;padding:0 11px;display:flex;align-items:center;gap:8px;transition:.12s}
.ctl:focus-within{border-color:var(--accent);box-shadow:0 0 0 2px var(--accent-wash)}
.ctl input,.ctl select{border:none;outline:none;flex:1;background:none;min-width:0}
.ctl select{cursor:pointer;appearance:none}
.ctl .chev{width:12px;height:12px;color:var(--faint);pointer-events:none}
.ctl .suffix{color:var(--muted);font-size:12px}
textarea.ctl{height:auto;min-height:60px;padding:8px 11px;resize:vertical;font-family:var(--mono);font-size:12.5px;display:block}
.field.invalid .ctl{border-color:var(--danger)}
.err-msg{font-size:12px;color:var(--danger);display:none}
.field.invalid .err-msg{display:block}
```

```html
<div class="field">
  <label><span class="req">*</span>服务名</label>
  <div class="ctl"><input placeholder="order-service"></div>
  <span class="err-msg">该项为必填</span>
</div>
<!-- 带后缀 --><div class="ctl"><input value="80" style="text-align:right"><span class="suffix">ms</span></div>
<!-- 下拉 --><div class="ctl"><select><option>HTTP</option></select><svg class="chev">…</svg></div>
```

### 10.2 按钮

```css
.btn{height:32px;padding:0 15px;border-radius:var(--radius-sm);font-size:13.5px;display:inline-flex;align-items:center;gap:6px;border:1px solid var(--border-2);background:#fff;color:var(--fg);transition:.12s}
.btn:hover{color:var(--accent);border-color:var(--accent)}
.btn.primary{background:var(--accent);color:#fff;border-color:var(--accent)}
.btn.primary:hover{background:var(--accent-hover);border-color:var(--accent-hover);color:#fff}
.btn.ghost{border-color:transparent;background:transparent;color:var(--fg-2)}
.btn.ghost:hover{background:#f0f2f5;color:var(--fg)}
.mini{width:34px;height:32px;border:1px solid var(--border-2);border-radius:var(--radius-sm);display:grid;place-items:center;color:var(--faint);background:#fff}
.mini:hover{color:var(--danger);border-color:var(--danger)}
.mini.primary{background:var(--accent);border-color:var(--accent);color:#fff}
```

```html
<button class="btn primary">保存</button>
<button class="btn">取消</button>
<button class="btn ghost">重置</button>
<button class="mini" title="删除"><svg>…</svg></button>
```

### 10.3 开关 `.sw`

```css
.sw{width:40px;height:22px;border-radius:11px;background:#bfbfbf;position:relative;flex-shrink:0;transition:.18s;cursor:pointer}
.sw::after{content:"";position:absolute;top:2px;left:2px;width:18px;height:18px;border-radius:50%;background:#fff;transition:.18s;box-shadow:0 1px 3px rgba(0,0,0,.3)}
.sw.on{background:var(--accent)}.sw.on::after{left:20px}
.sw.sm{width:32px;height:18px}.sw.sm::after{width:14px;height:14px}.sw.sm.on::after{left:16px}
.sw-row{display:flex;align-items:center;gap:10px}.sw-row .lab{font-size:13px;color:var(--fg-2)}
```

```html
<div class="sw-row"><div class="sw on" onclick="this.classList.toggle('on')"></div><span class="lab">启用规则</span></div>
<div class="sw sm on"></div> <!-- 小号 -->
```

### 10.4 步进器 `.stepper`

```css
.stepper{display:flex;border:1px solid var(--border-2);border-radius:var(--radius-sm);overflow:hidden;width:120px;height:34px}
.stepper button{width:34px;background:var(--raised);color:var(--fg-2);font-size:16px}
.stepper button:hover{color:var(--accent);background:#eef4ff}
.stepper input{flex:1;border:none;border-left:1px solid var(--border);border-right:1px solid var(--border);text-align:center;outline:none;font-family:var(--mono);font-variant-numeric:tabular-nums}
```

```html
<div class="stepper"><button>−</button><input value="3"><button>+</button></div>
```

### 10.5 单选 `.radio`

```css
.radio{display:inline-flex;align-items:center;gap:8px;font-size:13.5px;cursor:pointer;padding:8px 14px;border:1px solid var(--border-2);border-radius:var(--radius-sm)}
.radio.on{border-color:var(--accent);background:var(--accent-wash);color:var(--accent-active)}
.radio .rd{width:15px;height:15px;border-radius:50%;border:1.5px solid var(--faint);display:grid;place-items:center}
.radio.on .rd{border-color:var(--accent)}
.radio.on .rd::after{content:"";width:7px;height:7px;border-radius:50%;background:var(--accent)}
```

```html
<div class="radio-row">
  <label class="radio on"><span class="rd"></span>放通</label>
  <label class="radio"><span class="rd"></span>拒绝</label>
</div>
```

### 10.6 分段选择器 `.seg-pick` / `.rel-seg`

```css
.rel-seg,.seg-pick{display:inline-flex;border:1px solid var(--border-2);border-radius:var(--radius-sm);overflow:hidden}
.rel-seg button,.seg-pick button{padding:5px 14px;font-size:12.5px;color:var(--fg-2);font-family:var(--mono);font-weight:600}
.seg-pick button{font-family:var(--font);font-weight:400;padding:6px 14px}
.rel-seg button.on,.seg-pick button.on{background:var(--accent);color:#fff}
.rel-note{font-size:12px;color:var(--muted)}
```

```html
<div class="rel-bar">
  <div class="rel-seg"><button class="on">AND</button><button>OR</button></div>
  <span class="rel-note">需同时满足全部条件</span>
</div>
```

### 10.7 标签 / 筛选 chip

```css
.tag{display:inline-flex;align-items:center;gap:5px;height:26px;padding:0 9px;border-radius:var(--radius-sm);background:var(--accent-wash);color:var(--accent-active);font-size:12.5px;border:1px solid #bcd9ff;font-family:var(--mono)}
.tag .rm{cursor:pointer;opacity:.6}.tag .rm:hover{opacity:1}
.tag-add{height:26px;padding:0 9px;border:1px dashed var(--border-2);border-radius:var(--radius-sm);color:var(--muted);font-size:12.5px;background:#fff}
.tag-add:hover{border-color:var(--accent);color:var(--accent)}
.kchip{height:30px;padding:0 13px;border:1px solid var(--border-2);border-radius:999px;background:#fff;color:var(--fg-2);font-size:13px;display:inline-flex;align-items:center;gap:6px;cursor:pointer;transition:.12s}
.kchip:hover{color:var(--accent);border-color:var(--accent)}
.kchip.on{background:var(--accent);color:#fff;border-color:var(--accent);font-weight:500}
.kchip .cnt{font-family:var(--mono);font-size:11px;opacity:.62}
```

```html
<div class="tags">
  <span class="tag">x-env=gray<span class="rm">×</span></span>
  <button class="tag-add">+ 添加标签</button>
</div>
<div class="kchips">
  <button class="kchip on">全部<span class="cnt">42</span></button>
  <button class="kchip">路由<span class="cnt">12</span></button>
</div>
```

### 10.8 状态指示

```css
.dot{display:inline-flex;align-items:center;gap:6px;font-size:12.5px}
.dot::before{content:"";width:6px;height:6px;border-radius:50%;background:var(--success)}
.dot.off{color:var(--muted)}.dot.off::before{background:var(--faint)}
.wpill{display:inline-flex;align-items:center;gap:5px;padding:2px 9px;border-radius:20px;font-size:12px;font-weight:500}
.wpill.ok{background:var(--success-wash);color:#389e0d}
.wpill.bad{background:var(--danger-wash);color:#cf1322}
```

```html
<span class="dot">启用</span>
<span class="dot off">已停用</span>
<span class="wpill ok">合计 100%</span>
<span class="wpill bad">合计 80%</span>
```

### 10.9 添加入口

```css
.add-row{display:inline-flex;align-items:center;gap:6px;color:var(--accent);font-size:13px;padding:6px 0}
.add-row:hover{color:var(--accent-active)}
.add-block{font-size:13.5px;border:1px dashed var(--border-2);border-radius:var(--radius-sm);width:100%;justify-content:center;height:38px;display:inline-flex;align-items:center;gap:6px;color:var(--accent);background:#fff}
.add-block:hover{border-color:var(--accent)}
```

```html
<button class="add-row"><svg>+</svg>添加一行</button>
<button class="add-block"><svg>+</svg>新增子规则</button>
```

### 10.10 编号分段壳 `.sec`

承载以上控件的容器(规则配置的主组织单元):

```css
.sec{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);margin-bottom:16px;overflow:hidden}
.sec-h{display:flex;align-items:center;gap:10px;padding:14px 18px;border-bottom:1px solid var(--border)}
.sec-h .no{width:22px;height:22px;border-radius:50%;background:var(--accent);color:#fff;display:grid;place-items:center;font-size:12px;font-weight:600;font-family:var(--mono)}
.sec-h .tt{font-size:14.5px;font-weight:600}
.sec-h .hint{font-size:12px;color:var(--muted);font-weight:400}
.sec-b{padding:18px}
.fg{display:grid;grid-template-columns:1fr 1fr;gap:16px 20px}
.field{display:flex;flex-direction:column;gap:6px}
.field.full{grid-column:1/-1}
.field label{font-size:13px;color:var(--fg-2)}
.field label .req{color:var(--danger);margin-right:3px}
```

```html
<section class="sec">
  <div class="sec-h"><span class="no">1</span><span class="tt">基础信息</span><span class="hint">规则名与作用域</span></div>
  <div class="sec-b">
    <div class="fg">
      <div class="field"><label>规则名</label><div class="ctl"><input></div></div>
      <div class="field full"><label>描述</label><textarea class="ctl"></textarea></div>
    </div>
  </div>
</section>
```

## 相关页面
