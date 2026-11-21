---
title: Widgets
description: Build native macOS Desktop and Notification Center widgets in TSX with Tailwind, theme tokens, live dates, deep links, interactive buttons, and zero-Xcode packaging.
order: 24
group: platforms
---

# Widgets

Native Shell lets you author native **macOS Desktop and Notification Center widgets** (macOS 14 Sonoma and newer) directly in TSX using the same primitives, Tailwind utilities, and theme tokens as your main app—without creating or maintaining an Xcode project.

## How it works

1. You declare widgets in `nsh.json` under `"widgets"` and write each widget as a pure declarative TSX module exporting `defineWidget(...)` from `nsh/widget`.
2. Your main app pushes structured data (`Widget.update`) or scheduled timelines (`Widget.setTimeline`). Native Shell evaluates your widget TSX headlessly in both Light and Dark appearances into a compact, platform-neutral **Widget Tree IR** snapshot stored in your shared App Group container.
3. During `nsh package --target macos`, Native Shell automatically compiles and embeds a sandboxed WidgetKit extension (`Contents/PlugIns/<App>Widgets.appex`) backed by a native SwiftUI renderer (`NshWidgetRuntime`), signs it inside-out, and wires up App Group entitlements.
4. Because every color carries resolved Light and Dark RGBA values, widgets adapt instantaneously when the user switches macOS appearance, and natively support macOS **Tinted** and **Desktop Unfocused** modes.

## Declare widgets in `nsh.json`

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
      "description": "Watchlist price, daily change, and goal progress at a glance.",
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

- `id`: unique widget kind identifier (ASCII letters, digits, `-`, `_`, `.`).
- `entry`: relative path to the widget TSX module.
- `name` / `description`: displayed in the macOS Widget Gallery.
- `families`: any combination of `"small"`, `"medium"`, and `"large"`.
- `refreshInterval` (optional): timeline reload hint such as `"15m"`, `"1h"`, or `"1d"` (minimum 5 minutes).
- `network` (optional, default `false`): adds `com.apple.security.network.client` to the widget extension entitlements.
- `macos.widgets.appGroup` (optional): overrides the derived App Group identifier. When omitted and packaged with `--sign <Developer ID>`, Native Shell automatically derives `<TEAM_ID>.<id>.widgets`, which works for Developer ID and DMG distribution without a provisioning profile.

## Author a widget (`widgets/PortfolioWidget.tsx`)

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
            accessibilityLabel="Refresh quote"
            class="px-2 py-1 rounded-md bg-secondary"
          >
            <Text class="text-xs font-medium text-secondary-foreground">Refresh</Text>
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
                <Text class="text-xs text-primary">Open Chart</Text>
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

### Supported widget surface

Widgets are statically validated during `nsh check`, `nsh package`, and `nsh publish`:

- **Allowed elements**: `View`, `Text`, `Image`, `If`, `Each` (`nsh`), `Progress`, `Link` (`nsh/base`), `WidgetAction`, `WidgetDate` (`nsh/widget`), `t` (`nsh/i18n`), and Tailwind + theme tokens.
- **Visual hints**: pass `widget={{ accentable?: boolean, accentedRenderingMode?: "fullColor" | "accented" | "desaturated" | "template", containerBackground?: boolean }}` on host elements. Root backgrounds automatically lift to SwiftUI `.containerBackground(for: .widget)` for native macOS translucency and Tinted rendering.
- **Disallowed in widgets**: scrollable/editable or multi-window elements (`Input`, `Scroll`, `List`, `WebView`, `Video`, `Canvas`, `Shader`, `Window`, dialogs, editors), raw pointer/key handlers (`onPress`, `onPan*`, `onKey*`), and host-only modules (`nsh/desktop`, `nsh/mobile`, `nsh/updater`, `fs/promises`, `process`). Use `<WidgetAction>` for interactive buttons and `<Link>` for deep links.

## Push data and handle actions from your main app

```tsx
import { UrlScheme } from "nsh";
import { Widget } from "nsh/widget";

// Push live state to the widget
await Widget.update("portfolio", {
  symbol: "NVDA",
  price: "$142.80",
  changePct: "+3.15%",
  positive: true,
  goalProgress: 0.8,
  updatedAt: Date.now(),
});

// Listen for <WidgetAction> clicks (live via Darwin notifications + queued offline actions on launch)
const stop = Widget.on("action", async (event) => {
  if (event.widgetId === "portfolio" && event.action === "refresh") {
    const fresh = await fetchQuote(event.payload);
    await Widget.update("portfolio", fresh);
  }
});

// Handle widget deep links (<Link> and root `url`)
UrlScheme.on("open", (url) => {
  // Navigate to pulse://symbol/NVDA/chart
});
```

See the full API reference in [`nsh/widget`](../modules/nsh-widget.md).

## Preview and package

```sh
# Fast local preview harness with Small/Medium/Large cards,
# Light/Dark/Tinted/Unfocused simulation, and hot reload:
nsh run --widget portfolio

# Build a signed macOS .app with embedded WidgetKit .appex:
nsh package --target macos --sign "Developer ID Application: Your Org (ABCDE12345)" --run

# Verify the signed app and embedded widget extension:
codesign --verify --deep --strict --verbose=2 .nsh/out/*.app
```

> [!TIP]
> On macOS, **Developer ID + DMG** distribution requires an App Group prefixed with your Apple Team ID (`<TEAM_ID>.<suffix>`). Native Shell derives this automatically when you pass `--sign`. Local ad-hoc builds (`nsh package --target macos` without `--sign`) use `nsh.local.<id>.widgets` and work with the preview harness and local launches.

## OTA updates (`.nshpkg`)

`nsh publish` automatically validates and includes your compiled widget bundles inside signed `.nshpkg` updates. When an updated package activates, Native Shell re-evaluates your widgets with their persisted data and refreshes WidgetKit timelines.

| Change | Supported via `.nshpkg` OTA? | Details |
|---|---|---|
| Widget layout, copy, Tailwind styles, theme tokens, colors, icons/assets | **Yes** | Main app evaluates updated widget TSX, writes fresh IR + assets to App Group, and reloads timelines |
| Data formatting, conditional branches, deep link URLs, `onAction` handling | **Yes** | Updated JS runs immediately on next data push or app launch |
| Adding or removing a widget `id` | **Full app update** | Widget kinds are declared statically in the signed `.appex` binary |
| Changing `families`, static gallery `name`, or `description` | **Full app update** | Registered statically in `StaticConfiguration` inside `.appex` |
| Changing `appGroup`, `network` entitlement, or URL `schemes` | **Full app update** | Sealed by the macOS code signature and `Info.plist` |
