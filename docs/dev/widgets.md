# Native Widgets (`nsh/widget`)

Native Shell renders home-screen and desktop widgets declaratively from TSX using a **dual-appearance Widget Tree IR** pipeline so widget extensions stay fast, lightweight, and safe under OS memory budgets (macOS WidgetKit's 30 MB jetsam limit) while sharing your app's exact Tailwind utilities, design tokens, and `nsh/base` primitives.

## Architecture

1. **Core IR & configuration (`crates/nsh-core/src/widget/`)** — Platform-neutral schema with zero JS or platform dependencies:
   - `config.rs`: parses and validates `nsh.json` `"widgets"` entries (`id`, `entry`, `name`, `description`, `families`, `refreshInterval`, `network`) and `macos.widgets` (`appGroup`).
   - `ir.rs`: defines `WidgetSnapshot` (v1), `WidgetTimelineIrEntry`, `WidgetFamilyTree`, dual-appearance `WidgetColor { light, dark }`, `WidgetFill`, and `WidgetNode` (`Box`, `Text`, `Date`, `Image`, `Progress`, `Link`, `Action`) with hard budgets (`MAX_WIDGET_NODES = 256`, `MAX_WIDGET_TREE_DEPTH = 16`, `MAX_SNAPSHOT_BYTES = 256 KiB`). Uses flat wire representations so serialization stays compatible when `serde_json/arbitrary_precision` is enabled in workspace builds.
   - `components.rs`: Rust components `WidgetAction` and `WidgetDate`, plus `Prop::WidgetHints` stored in `Core::widget_nodes` (`FxHashMap<NodeId, WidgetNodeMeta>`) so non-widget apps add 0 bytes to `Node`.
   - `crates/nsh-core/src/runtime/widget.rs`: `#[module] Widget` (`available`, `update`, `setTimeline`, `reload`, `getData`, `on("action", ...)`) registered under `"nsh/widget"`.

2. **Static validation (`crates/nsh-bundler/src/widget_check.rs`)** — Oxc AST visitor run automatically during `nsh check`, `nsh package`, and `nsh publish`. Rejects interactive/scrollable elements (`Input`, `Scroll`, `List`, `WebView`, `Video`, `Canvas`, `Shader`, `Window`, dialogs, editors), forbidden event handlers (`onPress`, `onPan*`, `onKey*`, `onScroll`, `animation`, `transition`), and host-only imports (`nsh/desktop`, `nsh/mobile`, `nsh/updater`, `fs/promises`, `process`, `websocket`) with exact `file:line:col` diagnostics.

3. **Headless dual-appearance evaluator (`crates/nsh-engine/src/widget.rs`, `crates/nsh-core/src/core/widget.rs`)**:
   - Evaluates a widget TSX entry inside an isolated headless `Core<ApproxTextEngine>` + `nsh-js` context using the app's UI sets and theme tokens.
   - Commits once in `Appearance::Light` (`capture_widget_appearance`), then switches the **same shadow tree** to `Appearance::Dark` (preserving all `NodeId`s) and captures again.
   - Pairs Light and Dark resolved colors per node into `WidgetColor { light, dark }`, lifts the root view's background to `container_background`, and writes `snapshot.json`, `data.json`, and referenced assets atomically into the shared container (`WidgetStore`).

4. **macOS WidgetKit runtime (`crates/nsh-platform-macos/swift/widget/` & `src/widget.rs`)**:
   - `NshWidgetRuntime` (`WidgetIR.swift`, `WidgetRenderer.swift`, `WidgetStore.swift`, `WidgetIntent.swift`, `WidgetProvider.swift`): generic SwiftUI interpreter compiled as a static module with `-parse-as-library -enable-library-evolution`.
   - Maps `WidgetColor` to dynamic `NSColor(name:nil)` so macOS Light/Dark switches apply instantaneously without waking JS.
   - Uses `.containerBackground(for: .widget)`, `.widgetAccentable()`, `.widgetAccentedRenderingMode()` (macOS 15+), `Text(date, style:)`, `Link(destination:)`, and `Button(intent: NshWidgetActionIntent)` (macOS 14+ AppIntents).
   - `NshWidgetActionIntent` applies optional offline `patchJson`, appends a `WidgetActionEvent` to `widgets/actions.jsonl`, and posts a Darwin notification (`<appGroup>.action`) watched by `WidgetActionObserver` in the main app.
   - `NshWidgetHostBridge.swift` (compiled in `crates/nsh-platform-macos/build.rs`) bridges `WidgetCenter.shared.reloadTimelines(ofKind:)`, `reloadAllTimelines()`, and `FileManager.default.containerURL(forSecurityApplicationGroupIdentifier:)`.

5. **Zero-Xcode `.appex` packaging (`crates/nsh-cli/src/macos_widgets.rs`)**:
   - When `nsh.json` declares `"widgets"`, `nsh package --target macos` validates and compiles each widget, evaluates default placeholder snapshots into `default-snapshots.json`, generates `.nsh/build/macos-widgets/WidgetsMain.swift`, compiles it against `NshWidgetRuntime` with `xcrun --sdk macosx swiftc` (merging `arm64` + `x86_64` with `lipo` for `--arch universal`), and assembles `<App>.app/Contents/PlugIns/<Executable>Widgets.appex`.
   - Injects mandatory `com.apple.security.app-sandbox = true` and `com.apple.security.application-groups = [<group>]` (plus optional `com.apple.security.network.client = true`) into `.appex`, merges `<group>` into the main `.app` entitlements, signs inside-out with Hardened Runtime (`--options runtime --timestamp` when signed with a certificate), and verifies with `codesign --verify --deep --strict`.
   - Apps without `"widgets"` skip all widget build steps and incur zero runtime or binary overhead.

6. **OTA updates (`.nshpkg`)**:
   - `nsh-package` carries `FileKind::WidgetManifest`, `FileKind::WidgetSource`, and `FileKind::WidgetBytecode` (forward-compatible with format v1; older runtimes ignore unknown `FileKind` entries).
   - On OTA activation, `nsh-engine::updates` extracts updated widget files alongside assets and `nsh-macos-app` resynchronizes persisted widget data through the updated widget TSX so live snapshots refresh automatically.

## Local Development Preview

Run `nsh run --widget [id]` or `nsh dev --widget [id]` to open the live widget preview harness with Small/Medium/Large cards, Light/Dark/Tinted/Unfocused simulation toggles, placeholder/live state switching, and hot reload.

## Verification

```sh
cargo test -p nsh-core --lib widget
cargo test -p nsh-bundler --lib widget_check
cargo test -p nsh-engine --lib widget
cargo test -p nsh-cli macos_widgets          # includes Rust <-> Swift golden IR test
./target/debug/nsh check examples/widgets
./target/debug/nsh package examples/widgets --target macos
codesign --verify --deep --strict --verbose=2 examples/widgets/.nsh/out/*.app
```
