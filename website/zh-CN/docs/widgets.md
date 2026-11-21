---
title: 桌面小组件 (Widgets)
description: 使用 TSX、Tailwind、主题令牌、实时日期、深度链接与交互按钮构建原生 macOS 桌面与通知中心小组件，无需 Xcode 工程自动打包签名。
order: 24
group: platforms
---

# 桌面小组件 (Widgets)

Native Shell 支持直接使用 TSX 编写原生 **macOS 桌面与通知中心小组件**（macOS 14 Sonoma 及以上版本），复用主应用的原语组件、Tailwind 工具类和主题令牌，全程无需创建或维护 Xcode 工程。

## 工作原理

1. 在 `nsh.json` 中通过 `"widgets"` 声明小组件，每个小组件是一个纯声明式 TSX 模块，从 `nsh/widget` 导出 `defineWidget(...)`。
2. 主应用调用 `Widget.update` 推送实时数据或 `Widget.setTimeline` 编排时间线时，Native Shell 会在无头上下文中同时按浅色与深色外观求值小组件 TSX，生成轻量级跨平台 **Widget Tree IR** 快照并原子写入共享 App Group 容器。
3. 执行 `nsh package --target macos` 时，Native Shell 自动使用原生 SwiftUI 渲染运行时（`NshWidgetRuntime`）编译并嵌入沙盒化的 WidgetKit 扩展（`Contents/PlugIns/<App>Widgets.appex`），配置 App Group 权限并完成从内到外的代码签名。
4. 快照中的每个颜色都同时包含浅色与深色 sRGB 值，因此用户切换系统外观时小组件会立即原生响应，并自动适配 macOS **染色模式（Tinted）** 与 **桌面失焦模式（Unfocused）**。

## 在 `nsh.json` 中声明小组件

```json
{
  "name": "Pulse",
  "id": "com.example.pulse",
  "version": "1.0.0",
  "build": 1,
  "schemes": ["pulse"],
  "widgets": [
    {
      "id": "portfolio",
      "entry": "widgets/PortfolioWidget.tsx",
      "name": "Portfolio",
      "description": "一览自选价格、当日涨跌幅与目标进度。",
      "families": ["small", "medium", "large"],
      "refreshInterval": "15m"
    }
  ],
  "macos": {
    "widgets": {
      "appGroup": "ABCDE12345.com.example.pulse.widgets"
    }
  }
}
```

- `id`：小组件唯一标识（ASCII 字母、数字、`-`、`_`、`.`）。
- `entry`：小组件 TSX 入口相对路径。
- `name` / `description`：在 macOS 小组件库中展示的名称与简介。
- `families`：支持的尺寸列表，可选 `"small"`、`"medium"`、`"large"`。
- `refreshInterval`（可选）：时间线刷新提示，如 `"15m"`、`"1h"`、`"1d"`（最短 5 分钟）。
- `network`（可选，默认 `false`）：为小组件扩展开启 `com.apple.security.network.client` 权限。
- `macos.widgets.appGroup`（可选）：自定义共享 App Group 标识。若省略且使用 `--sign <Developer ID>` 打包，Native Shell 会自动推导 `<TEAM_ID>.<id>.widgets`，支持 Developer ID + DMG 分发且无需 provisioning profile。

## 编写小组件（`widgets/PortfolioWidget.tsx`）

```tsx
import { Text, View } from "nsh";
import { Link, Progress } from "nsh/base";
import { defineWidget, WidgetAction, WidgetDate } from "nsh/widget";

interface PortfolioData {
  symbol: string;
  price: string;
  changePct: string;
  positive: boolean;
  goalProgress: number;
  updatedAt: number;
}

export default defineWidget<PortfolioData>({
  placeholder: {
    symbol: "AAPL",
    price: "$218.40",
    changePct: "+1.42%",
    positive: true,
    goalProgress: 0.72,
    updatedAt: Date.now(),
  },
  url: ({ data }) => `pulse://symbol/${(data ?? this.placeholder!).symbol}`,
  render({ data, family }) {
    const item = data ?? this.placeholder!;
    return (
      <View class="flex-1 flex-col justify-between p-4 bg-card">
        <View class="flex-row items-center justify-between">
          <Text class="text-sm font-semibold text-foreground">{item.symbol}</Text>
          <WidgetAction
            action="refresh"
            payload={{ symbol: item.symbol }}
            accessibilityLabel="刷新行情"
            class="px-2 py-1 rounded-md bg-secondary"
          >
            <Text class="text-xs font-medium text-secondary-foreground">刷新</Text>
          </WidgetAction>
        </View>

        <View class="flex-col gap-1">
          <Text class="text-2xl font-bold text-foreground">{item.price}</Text>
          <Text
            class={item.positive ? "text-xs font-semibold text-emerald-500" : "text-xs font-semibold text-destructive"}
            widget={{ accentable: true }}
          >
            {item.changePct}
          </Text>
        </View>

        {family !== "small" && (
          <View class="flex-col gap-2">
            <Progress value={Math.round(item.goalProgress * 100)} class="h-1.5 rounded-full bg-muted" />
            <View class="flex-row items-center justify-between">
              <Link href={`pulse://symbol/${item.symbol}/chart`} class="text-xs text-primary">
                <Text class="text-xs text-primary">查看图表</Text>
              </Link>
              <WidgetDate date={item.updatedAt} mode="relative" class="text-xs text-muted-foreground" />
            </View>
          </View>
        )}
      </View>
    );
  },
});
```

### 小组件可用能力与静态校验

`nsh check`、`nsh package` 和 `nsh publish` 会自动对小组件入口执行静态检查：

- **允许使用的组件与能力**：`View`、`Text`、`Image`、`If`、`Each`（`nsh`），`Progress`、`Link`（`nsh/base`），`WidgetAction`、`WidgetDate`（`nsh/widget`），`t`（`nsh/i18n`），以及 Tailwind 和主题令牌。
- **视觉提示**：可在宿主元素上传入 `widget={{ accentable?: boolean, accentedRenderingMode?: "fullColor" | "accented" | "desaturated" | "template", containerBackground?: boolean }}`。根节点背景会自动提升为 SwiftUI `.containerBackground(for: .widget)`，自然融入 macOS 桌面半透明与染色渲染效果。
- **禁止使用**：滚动、输入或多窗口类组件（`Input`、`Scroll`、`List`、`WebView`、`Video`、`Canvas`、`Shader`、`Window`、弹窗、编辑器等），常规手势/按键事件（`onPress`、`onPan*`、`onKey*`），以及主应用专属模块（`nsh/desktop`、`nsh/mobile`、`nsh/updater`、`fs/promises`、`process`）。交互按钮请使用 `<WidgetAction>`，跳转请使用 `<Link>`。

## 从主应用推送数据并响应交互

```tsx
import { UrlScheme } from "nsh";
import { Widget } from "nsh/widget";

// 推送实时数据到小组件
await Widget.update("portfolio", {
  symbol: "NVDA",
  price: "$142.80",
  changePct: "+3.15%",
  positive: true,
  goalProgress: 0.8,
  updatedAt: Date.now(),
});

// 监听桌面小组件上的 <WidgetAction> 点击（运行中实时到达，离线点击在下次启动时补发）
const stop = Widget.on("action", async (event) => {
  if (event.widgetId === "portfolio" && event.action === "refresh") {
    const fresh = await fetchQuote(event.payload);
    await Widget.update("portfolio", fresh);
  }
});

// 处理小组件深度链接（<Link> 或根节点 `url`）
UrlScheme.on("open", (url) => {
  // 跳转到 pulse://symbol/NVDA/chart
});
```

完整 API 参考见 [`nsh/widget`](../modules/nsh-widget.md)。

## 本地预览与打包

```sh
# 启动本地小组件预览面板（支持 Small/Medium/Large 尺寸、
# 浅色/深色/染色/失焦模式模拟与热重载）：
nsh run --widget portfolio

# 打包带 WidgetKit .appex 扩展的已签名 macOS .app：
nsh package --target macos --sign "Developer ID Application: Your Org (ABCDE12345)" --run

# 验证主应用与内嵌小组件扩展的签名完整性：
codesign --verify --deep --strict --verbose=2 .nsh/out/*.app
```

> [!TIP]
> 在 macOS 上使用 **Developer ID + DMG** 分发时，App Group 必须以 Apple Team ID 为前缀（`<TEAM_ID>.<suffix>`）。传入 `--sign` 时 Native Shell 会自动推导该前缀；未传 `--sign` 的本地 ad-hoc 打包会回退到 `nsh.local.<id>.widgets`。

## OTA 热更新（`.nshpkg`）

`nsh publish` 会自动校验并将编译后的小组件模块打包进已签名的 `.nshpkg`。新版本激活后，Native Shell 会使用已持久化的业务数据自动重新求值所有小组件并刷新 WidgetKit 时间线。

| 变更类型 | 是否支持 `.nshpkg` OTA？ | 说明 |
|---|---|---|
| 小组件布局、文案、Tailwind 样式、主题令牌、配色、图标与静态资源 | **支持** | 主应用用新 TSX 重新求值并原子写入 App Group，随后刷新时间线 |
| 数据格式化、条件分支、深度链接 URL、`onAction` 处理逻辑 | **支持** | 新 JS 在下次数据推送或启动时立即生效 |
| 新增或删除小组件 `id` | **需整包更新** | `WidgetBundle` 中的小组件类型静态编译在已签名的 `.appex` 内 |
| 修改 `families`、小组件库 `name` 或 `description` | **需整包更新** | 静态声明在 `.appex` 的 `StaticConfiguration` 中 |
| 修改 `appGroup`、`network` 权限或 URL `schemes` | **需整包更新** | 受 macOS 代码签名与 `Info.plist` 封缄保护 |
