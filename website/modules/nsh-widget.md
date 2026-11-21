---
title: nsh/widget
description: "Declarative native widgets: defineWidget, WidgetAction, WidgetDate, and the main-app Widget API for pushing snapshots, scheduling timelines, reloading, and handling interactive actions."
order: 15
---

# nsh/widget

`nsh/widget` provides both the authoring primitives used inside widget entry modules (`defineWidget`, `WidgetAction`, `WidgetDate`) and the main-app runtime API (`Widget`) for syncing data, scheduling timelines, and handling interactive desktop widget button actions. See the [Widgets guide](../docs/widgets.md) for `nsh.json` configuration, previewing, and macOS packaging.

```tsx
import { Widget, defineWidget, WidgetAction, WidgetDate } from "nsh/widget";
```

## Main-app API: `Widget`

### `Widget.available()`

```ts
Widget.available(): Promise<boolean>
```

Resolves to `true` when the running app is packaged with a native widget host (for example a macOS `.app` bundle with widgets declared in `nsh.json`), and `false` otherwise.

### `Widget.update(widgetId, data, options?)`

```ts
Widget.update(
  widgetId: string,
  data: unknown,
  options?: { reloadPolicy?: "atEnd" | "never" | { after: number } }
): Promise<void>
```

Evaluates the declared widget `widgetId` across all of its configured `families` in both Light and Dark appearances, writes the resulting snapshot and referenced assets atomically to the shared App Group container, persists `data`, and reloads WidgetKit timelines for `widgetId`.

### `Widget.setTimeline(widgetId, entries, options?)`

```ts
Widget.setTimeline(
  widgetId: string,
  entries: Array<{ date: number; data: unknown; relevance?: number }>,
  options?: { reloadPolicy?: "atEnd" | "never" | { after: number } }
): Promise<void>
```

Schedules one or more future timeline entries (with Unix millisecond timestamps `date` and optional Smart Stack `relevance` in `0..1`) in a single snapshot so WidgetKit advances through them natively without waking your app.

### `Widget.reload(widgetId?)`

```ts
Widget.reload(widgetId?: string): Promise<void>
```

Requests an OS timeline reload for one widget `widgetId`, or all widgets of the app when omitted.

### `Widget.getData(widgetId)`

```ts
Widget.getData<T = unknown>(widgetId: string): Promise<T | null>
```

Reads the most recently persisted JSON payload for `widgetId` from the shared container (including any offline `<WidgetAction patch={...} />` updates), or `null` if no data has been written yet.

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

Subscribes to interactive `<WidgetAction>` clicks from macOS 14+ desktop and Notification Center widgets. Actions arrive immediately while the app is running (via Darwin notifications), and any actions triggered while the app was closed are drained and delivered on launch. Returns an unsubscribe function.

## Authoring API

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

Registers the default export of a widget TSX entry file.

### `WidgetAction`

```tsx
<WidgetAction
  action="refresh"
  payload={{ symbol: "AAPL" }}
  patch={{ stale: false }}
  accessibilityLabel="Refresh quote"
  class="px-2 py-1 rounded-md bg-secondary"
>
  <Text class="text-xs font-medium text-secondary-foreground">Refresh</Text>
</WidgetAction>
```

Interactive button rendered natively via SwiftUI `Button(intent:)` (macOS 14+ AppIntents). Clicking triggers `Widget.on("action", ...)` in the main app; when `patch` is supplied, its top-level JSON keys are merged into the widget's shared data file immediately for offline feedback.

### `WidgetDate`

```tsx
<WidgetDate
  date={item.updatedAt}
  mode="relative" // "relative" | "timer" | "offset" | "time" | "date"
  class="text-xs text-muted-foreground"
/>
```

OS-driven live date or countdown text rendered via SwiftUI `Text(date, style:)` so it ticks natively inside WidgetKit without consuming timeline reload budget.
