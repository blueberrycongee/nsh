---
title: nsh/widget
description: "声明式原生小组件模块：defineWidget、WidgetAction、WidgetDate，以及用于推送快照、编排时间线、触发刷新和监听交互按钮的主应用 Widget API。"
order: 15
---

# nsh/widget

`nsh/widget` 提供两类 API：小组件入口模块的声明式构建原语（`defineWidget`、`WidgetAction`、`WidgetDate`），以及主应用用于同步数据、编排时间线和接收桌面交互按钮事件的 `Widget` 运行时 API。配置、预览与 macOS 打包流程请参考 [桌面小组件指南](../docs/widgets.md)。

```tsx
import { Widget, defineWidget, WidgetAction, WidgetDate } from "nsh/widget";
```

## 主应用 API：`Widget`

### `Widget.available()`

```ts
Widget.available(): Promise<boolean>
```

当前进程打包了原生小组件宿主（例如包含小组件声明的 macOS `.app`）时解析为 `true`，否则解析为 `false`。

### `Widget.update(widgetId, data, options?)`

```ts
Widget.update(
  widgetId: string,
  data: unknown,
  options?: { reloadPolicy?: "atEnd" | "never" | { after: number } }
): Promise<void>
```

按浅色与深色双外观对 `widgetId` 的所有已配置尺寸求值，将快照与引用的图片资源原子写入共享 App Group 容器，持久化 `data`，并通知 WidgetKit 刷新 `widgetId` 的时间线。

### `Widget.setTimeline(widgetId, entries, options?)`

```ts
Widget.setTimeline(
  widgetId: string,
  entries: Array<{ date: number; data: unknown; relevance?: number }>,
  options?: { reloadPolicy?: "atEnd" | "never" | { after: number } }
): Promise<void>
```

一次性编排多个未来时间节点（`date` 为 Unix 毫秒时间戳，`relevance` 为可选的 `0..1` 智能叠放权重），让 WidgetKit 按时自动切换内容而无需唤醒主应用。

### `Widget.reload(widgetId?)`

```ts
Widget.reload(widgetId?: string): Promise<void>
```

通知系统刷新指定 `widgetId` 的时间线；省略参数时刷新应用的所有小组件。

### `Widget.getData(widgetId)`

```ts
Widget.getData<T = unknown>(widgetId: string): Promise<T | null>
```

从共享容器读取 `widgetId` 最近一次持久化的 JSON 数据（包含离线 `<WidgetAction patch={...} />` 合并的字段）；尚未写入过数据时返回 `null`。

### `Widget.on("action", handler)`

```ts
Widget.on(
  event: "action",
  handler: (event: {
    widgetId: string;
    action: string;
    payload?: unknown;
    timestamp: number;
  }) => void
): () => void
```

订阅来自 macOS 14+ 桌面与通知中心小组件的 `<WidgetAction>` 按钮点击事件。主应用运行中会通过 Darwin 通知实时收到事件；主应用关闭期间产生的交互事件会在下次启动时自动补发。返回取消订阅函数。

## 小组件编写 API

### `defineWidget(definition)`

```ts
interface WidgetRenderProps<T = unknown> {
  data: T | undefined;
  family: "small" | "medium" | "large";
  isPlaceholder: boolean;
  date: number;
}

interface WidgetDefinition<T = unknown> {
  render: (props: WidgetRenderProps<T>) => JSX.Element;
  placeholder?: T;
  url?: (props: WidgetRenderProps<T>) => string | undefined;
}

function defineWidget<T>(definition: WidgetDefinition<T>): WidgetDefinition<T>;
```

声明小组件 TSX 入口文件的默认导出定义。

### `WidgetAction`

```tsx
<WidgetAction
  action="refresh"
  payload={{ symbol: "AAPL" }}
  patch={{ stale: false }}
  accessibilityLabel="刷新行情"
  class="px-2 py-1 rounded-md bg-secondary"
>
  <Text class="text-xs font-medium text-secondary-foreground">刷新</Text>
</WidgetAction>
```

在 macOS 14+ 上原生映射为 SwiftUI `Button(intent:)`（AppIntents）的交互按钮。点击后会向主应用派发 `Widget.on("action", ...)`；传入 `patch` 时会立即把顶层字段合并进共享数据文件以提供离线即时反馈。

### `WidgetDate`

```tsx
<WidgetDate
  date={item.updatedAt}
  mode="relative" // "relative" | "timer" | "offset" | "time" | "date"
  class="text-xs text-muted-foreground"
/>
```

由系统驱动的实时日期或倒计时文本，原生映射为 SwiftUI `Text(date, style:)`，可在 WidgetKit 内持续自动跳动且不消耗时间线刷新配额。
