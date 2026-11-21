//! Headless TSX → [`WidgetSnapshot`] evaluator, atomic App Group / local
//! [`WidgetStore`], and [`EngineWidgetHost`] implementation for `nsh/widget`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use nsh_core::{
    ApproxTextEngine, Core, JsonValue,
    element::{Environment as CoreEnvironment, Registry, Tokens},
    platform::{Appearance, ColorScheme, Environment as JsEnvironment, Os},
    widget::{
        WIDGET_IR_VERSION, WidgetActionEvent, WidgetEntryConfig, WidgetFamily, WidgetFamilyTree,
        WidgetHost, WidgetImageSource, WidgetNode, WidgetReloadPolicy, WidgetSnapshot,
        WidgetTimelineDataEntry, WidgetTimelineIrEntry,
    },
};

/// A compiled widget module ready for headless IR evaluation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WidgetBundle {
    /// Declarative configuration from `nsh.json`.
    pub config: WidgetEntryConfig,
    /// Bundled ES module JS source (exporting `default` from `defineWidget(...)`).
    pub code: String,
    /// Optional asset directory for resolving `asset:/<path>` images referenced by the widget.
    pub assets_dir: Option<PathBuf>,
}

impl WidgetBundle {
    /// Creates a new compiled widget bundle.
    pub fn new(config: WidgetEntryConfig, code: String) -> Self {
        Self {
            config,
            code,
            assets_dir: None,
        }
    }

    /// Sets the asset directory for copying referenced widget images into the shared container.
    pub fn with_assets_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.assets_dir = dir;
        self
    }
}

/// Evaluates widget TSX bundles into dual-appearance [`WidgetSnapshot`] IR
/// using NSH's headless [`Core`] + QuickJS host.
#[derive(Clone)]
pub struct WidgetEvaluator {
    bundles: Arc<RwLock<BTreeMap<String, WidgetBundle>>>,
    ui_sets: Arc<Vec<Registry>>,
    tokens: Arc<RwLock<Option<Tokens>>>,
}

impl Default for WidgetEvaluator {
    fn default() -> Self {
        Self::new(Vec::new(), None)
    }
}

#[allow(
    clippy::field_reassign_with_default,
    reason = "core IR structs are #[non_exhaustive]"
)]
impl WidgetEvaluator {
    /// Creates an evaluator with the app's UI sets (e.g. `nsh_base::registry()`)
    /// and optional theme [`Tokens`].
    pub fn new(ui_sets: Vec<Registry>, tokens: Option<Tokens>) -> Self {
        Self {
            bundles: Arc::new(RwLock::new(BTreeMap::new())),
            ui_sets: Arc::new(ui_sets),
            tokens: Arc::new(RwLock::new(tokens)),
        }
    }

    /// Registers or replaces a compiled widget bundle (e.g. on startup or OTA update).
    pub fn register(&self, bundle: WidgetBundle) {
        self.bundles
            .write()
            .expect("widget bundles lock")
            .insert(bundle.config.id.clone(), bundle);
    }

    /// Updates the theme [`Tokens`] used during widget evaluation.
    pub fn set_tokens(&self, tokens: Option<Tokens>) {
        *self.tokens.write().expect("widget tokens lock") = tokens;
    }

    /// Returns all registered widget ids in deterministic order.
    pub fn widget_ids(&self) -> Vec<String> {
        self.bundles
            .read()
            .expect("widget bundles lock")
            .keys()
            .cloned()
            .collect()
    }

    /// Evaluates a single data entry for `widget_id` into a [`WidgetSnapshot`].
    pub fn evaluate_single(
        &self,
        widget_id: &str,
        data: JsonValue,
        policy: WidgetReloadPolicy,
        now_ms: i64,
    ) -> Result<WidgetSnapshot, String> {
        let mut entry = WidgetTimelineDataEntry::default();
        entry.date_ms = now_ms;
        entry.data = data;
        self.evaluate_timeline(widget_id, &[entry], policy, now_ms, false)
    }

    /// Evaluates the placeholder/default snapshot for `widget_id` (passing
    /// `isPlaceholder: true` and `data: undefined` so the widget uses its
    /// `placeholder` definition).
    pub fn evaluate_placeholder(
        &self,
        widget_id: &str,
        now_ms: i64,
    ) -> Result<WidgetSnapshot, String> {
        let mut entry = WidgetTimelineDataEntry::default();
        entry.date_ms = now_ms;
        entry.data = JsonValue::Null;
        self.evaluate_timeline(widget_id, &[entry], WidgetReloadPolicy::AtEnd, now_ms, true)
    }

    /// Evaluates a multi-entry timeline for `widget_id` into a validated
    /// [`WidgetSnapshot`].
    pub fn evaluate_timeline(
        &self,
        widget_id: &str,
        entries: &[WidgetTimelineDataEntry],
        policy: WidgetReloadPolicy,
        now_ms: i64,
        is_placeholder: bool,
    ) -> Result<WidgetSnapshot, String> {
        if entries.is_empty() {
            return Err(format!(
                "widget {widget_id:?}: timeline must contain at least one entry"
            ));
        }
        let bundle = self
            .bundles
            .read()
            .expect("widget bundles lock")
            .get(widget_id)
            .cloned()
            .ok_or_else(|| format!("unknown widget id {widget_id:?} (not declared in nsh.json)"))?;

        let tokens = self.tokens.read().expect("widget tokens lock").clone();
        let mut ir_entries = Vec::with_capacity(entries.len());

        for entry in entries {
            let mut families = BTreeMap::new();
            for &family in &bundle.config.families {
                let tree = eval_family(
                    &bundle,
                    &self.ui_sets,
                    tokens.as_ref(),
                    family,
                    &entry.data,
                    entry.date_ms,
                    is_placeholder,
                )?;
                families.insert(family, tree);
            }
            let mut ir_entry = WidgetTimelineIrEntry::default();
            ir_entry.date_ms = entry.date_ms;
            ir_entry.relevance = entry.relevance;
            ir_entry.families = families;
            ir_entries.push(ir_entry);
        }

        let (reload_after_ms, reload_never) = match policy {
            WidgetReloadPolicy::Never => (None, true),
            WidgetReloadPolicy::After(ms) => (Some(ms), false),
            _ => {
                let default_ms = bundle
                    .config
                    .refresh_interval_secs()
                    .ok()
                    .flatten()
                    .map(|(secs, _)| (secs as i64).saturating_mul(1000));
                (default_ms, false)
            }
        };

        let mut snapshot = WidgetSnapshot::default();
        snapshot.version = WIDGET_IR_VERSION;
        snapshot.widget_id = widget_id.to_owned();
        snapshot.generated_at_ms = now_ms;
        snapshot.reload_after_ms = reload_after_ms;
        snapshot.reload_never = reload_never;
        snapshot.entries = ir_entries;

        snapshot.validate().map_err(|e| e.to_string())?;
        Ok(snapshot)
    }

    /// Returns the asset directory configured for `widget_id`, if any.
    pub fn assets_dir(&self, widget_id: &str) -> Option<PathBuf> {
        self.bundles
            .read()
            .expect("widget bundles lock")
            .get(widget_id)
            .and_then(|b| b.assets_dir.clone())
    }
}

/// Evaluates one `(widget, family, entry)` combination in an isolated headless
/// QuickJS + `Core` session, running Light and Dark passes over the same shadow
/// tree to produce dual-appearance [`WidgetColor`] values.
#[allow(
    clippy::field_reassign_with_default,
    reason = "Environment is #[non_exhaustive]"
)]
fn eval_family(
    bundle: &WidgetBundle,
    ui_sets: &[Registry],
    tokens: Option<&Tokens>,
    family: WidgetFamily,
    data: &JsonValue,
    date_ms: i64,
    is_placeholder: bool,
) -> Result<WidgetFamilyTree, String> {
    let size = family.macos_size();
    let mut core = Core::new(ApproxTextEngine, size);
    let sets: Vec<_> = nsh_core::runtime::registries()
        .iter()
        .chain(ui_sets)
        .cloned()
        .collect();
    core.set_components(Registry::merged(&sets));
    if let Some(tokens) = tokens {
        core.set_host_tokens(Some(tokens.clone()));
    }

    let mut core_env = CoreEnvironment::default();
    core_env.os = Os::MacOs;
    core_env.window = size;
    core_env.pixel_ratio = 2.0;
    core_env.dark = false;
    core.set_environment(core_env);
    core.set_appearance(Appearance::Light);

    let mut js_env = JsEnvironment::default();
    js_env.os = Os::MacOs;
    js_env.window = size;
    js_env.pixel_ratio = 2.0;
    js_env.color_scheme = ColorScheme::Light;

    let options = nsh_js::HostOptions {
        ui_sets: sets,
        ..nsh_js::HostOptions::default()
    };
    let host = nsh_js::Host::with_options(options)
        .map_err(|e| format!("create headless JS host for widget: {e}"))?;
    host.set_environment(&js_env);
    host.set_tokens(core.tokens().clone());

    // Harness wrapper: imports the widget module chunk, mounts `def.render(props)`,
    // and stores `def.url?.(props)` on `globalThis.__nsh_widget_url`.
    let data_json = if is_placeholder && data.is_null() {
        "undefined".to_owned()
    } else {
        serde_json::to_string(data).map_err(|e| format!("serialize widget data: {e}"))?
    };
    let family_str = family.as_str();
    let harness = format!(
        "import {{ mount }} from \"nsh\";\n\
         import def from \"./widget.js\";\n\
         const rawData = {data_json};\n\
         const props = {{\n\
           data: rawData === undefined ? def.placeholder : rawData,\n\
           family: {family_str:?},\n\
           isPlaceholder: {is_placeholder},\n\
           date: {date_ms},\n\
         }};\n\
         globalThis.__nsh_widget_url = typeof def.url === \"function\" ? (def.url(props) ?? null) : null;\n\
         mount(() => def.render(props));\n"
    );

    host.add_chunk_source("widget.js", bundle.code.clone());
    host.eval_source(&harness)
        .map_err(|e| format!("evaluate widget {:?} ({family_str}): {e}", bundle.config.id))?;

    // Drain JS ops into Core and commit Light pass.
    drain_and_commit(&host, &mut core)?;
    let light = core.capture_widget_appearance();

    // Switch to Dark appearance on the SAME shadow tree (identical NodeIds!)
    // so theme tokens and `dark:` classes re-resolve cleanly.
    core.set_appearance(Appearance::Dark);
    let mut dark_core_env = core_env;
    dark_core_env.dark = true;
    core.set_environment(dark_core_env);
    let mut dark_js_env = js_env;
    dark_js_env.color_scheme = ColorScheme::Dark;
    host.set_environment(&dark_js_env);
    host.set_tokens(core.tokens().clone());
    drain_and_commit(&host, &mut core)?;
    let dark = core.capture_widget_appearance();

    // Extract optional root deep link URL returned by `def.url(props)`.
    let url = host
        .global_string("__nsh_widget_url")
        .filter(|s| !s.is_empty());

    Ok(core.extract_widget_family_tree(&light, &dark, url))
}

fn drain_and_commit(host: &nsh_js::Host, core: &mut Core<ApproxTextEngine>) -> Result<(), String> {
    let mut ops = Vec::new();
    let mut events = Vec::new();
    for _ in 0..8 {
        host.take_ops(&mut ops);
        if !ops.is_empty() {
            for op in ops.drain(..) {
                core.apply(op)
                    .map_err(|e| format!("apply widget op: {e:?}"))?;
            }
        }
        let _ = core.commit();
        events.clear();
        core.take_events(&mut events);
        if events.is_empty() {
            break;
        }
        for event in events.drain(..) {
            host.deliver(event);
        }
    }
    Ok(())
}

/// Atomic file store inside a shared App Group container (or local fallback
/// directory) used by both the main NSH app and the macOS WidgetKit extension.
#[derive(Debug, Clone)]
pub struct WidgetStore {
    root: PathBuf,
}

impl WidgetStore {
    /// Creates a store rooted at `container_dir` (files are stored under
    /// `<container_dir>/widgets/`).
    pub fn new(container_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: container_dir.into().join("widgets"),
        }
    }

    /// Root widgets directory inside the container.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to a widget's snapshot JSON file (`widgets/<id>/snapshot.json`).
    pub fn snapshot_path(&self, widget_id: &str) -> PathBuf {
        self.root.join(widget_id).join("snapshot.json")
    }

    /// Path to a widget's persisted raw data JSON file (`widgets/<id>/data.json`).
    pub fn data_path(&self, widget_id: &str) -> PathBuf {
        self.root.join(widget_id).join("data.json")
    }

    /// Path to the queued offline widget actions file (`widgets/actions.jsonl`).
    pub fn actions_path(&self) -> PathBuf {
        self.root.join("actions.jsonl")
    }

    /// Path to copied widget image assets (`widgets/assets/`).
    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    /// Writes `snapshot` atomically (`snapshot.json.tmp` → `rename`) and copies
    /// any referenced `Asset` images from `source_assets_dir` into the container.
    pub fn write_snapshot(
        &self,
        snapshot: &WidgetSnapshot,
        source_assets_dir: Option<&Path>,
    ) -> Result<(), String> {
        snapshot.validate().map_err(|e| e.to_string())?;
        if let Some(src_dir) = source_assets_dir {
            let mut assets = BTreeSet::new();
            collect_snapshot_assets(snapshot, &mut assets);
            for rel in assets {
                if rel.is_empty() || rel.contains("..") || rel.starts_with('/') {
                    continue;
                }
                let src = src_dir.join(&rel);
                if src.is_file() {
                    let dst = self.assets_dir().join(&rel);
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("create widget asset dir: {e}"))?;
                    }
                    let tmp = dst.with_extension("tmp");
                    fs::copy(&src, &tmp).map_err(|e| format!("copy widget asset {rel}: {e}"))?;
                    fs::rename(&tmp, &dst)
                        .map_err(|e| format!("commit widget asset {rel}: {e}"))?;
                }
            }
        }
        let bytes =
            serde_json::to_vec(snapshot).map_err(|e| format!("serialize widget snapshot: {e}"))?;
        atomic_write(&self.snapshot_path(&snapshot.widget_id), &bytes)
    }

    /// Reads the persisted [`WidgetSnapshot`] for `widget_id`, if present.
    pub fn read_snapshot(&self, widget_id: &str) -> Result<Option<WidgetSnapshot>, String> {
        let path = self.snapshot_path(widget_id);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let snapshot =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(Some(snapshot))
    }

    /// Persists the raw JSON `data` for `widget_id` atomically (`data.json`).
    pub fn write_data(&self, widget_id: &str, data: &JsonValue) -> Result<(), String> {
        let bytes = serde_json::to_vec(data).map_err(|e| format!("serialize widget data: {e}"))?;
        atomic_write(&self.data_path(widget_id), &bytes)
    }

    /// Reads the last persisted raw JSON `data` for `widget_id`.
    pub fn read_data(&self, widget_id: &str) -> Result<Option<JsonValue>, String> {
        let path = self.data_path(widget_id);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let data =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(Some(data))
    }

    /// Appends a [`WidgetActionEvent`] to `actions.jsonl` (used by tests and
    /// extensions).
    pub fn enqueue_action(&self, event: &WidgetActionEvent) -> Result<(), String> {
        let path = self.actions_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create widget store dir: {e}"))?;
        }
        let line = serde_json::to_string(event).map_err(|e| format!("serialize action: {e}"))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        writeln!(file, "{line}").map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(())
    }

    /// Atomically drains and clears all queued [`WidgetActionEvent`] entries
    /// from `actions.jsonl`.
    pub fn drain_actions(&self) -> Vec<WidgetActionEvent> {
        let path = self.actions_path();
        if !path.is_file() {
            return Vec::new();
        }
        let draining = path.with_extension(format!("draining-{}", std::process::id()));
        if fs::rename(&path, &draining).is_err() {
            return Vec::new();
        }
        let content = fs::read_to_string(&draining).unwrap_or_default();
        let _ = fs::remove_file(&draining);
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<WidgetActionEvent>(line).ok())
            .collect()
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create directory {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))?;
    Ok(())
}

fn collect_snapshot_assets(snapshot: &WidgetSnapshot, out: &mut BTreeSet<String>) {
    for entry in &snapshot.entries {
        for tree in entry.families.values() {
            collect_node_assets(&tree.root, out);
        }
    }
}

fn collect_node_assets(node: &WidgetNode, out: &mut BTreeSet<String>) {
    match node {
        WidgetNode::Box { children, .. } => {
            for child in children {
                collect_node_assets(child, out);
            }
        }
        WidgetNode::Link { child, .. } | WidgetNode::Action { child, .. } => {
            collect_node_assets(child, out);
        }
        WidgetNode::Image {
            source: WidgetImageSource::Asset { path },
            ..
        } => {
            out.insert(path.clone());
        }
        WidgetNode::Text { .. }
        | WidgetNode::Date { .. }
        | WidgetNode::Image { .. }
        | WidgetNode::Progress { .. }
        | _ => {}
    }
}

/// Callback invoked after persisting a snapshot to ask the OS widget host
/// (e.g. macOS `WidgetCenter`) to reload one widget kind or all kinds.
pub type ReloadHook = Arc<dyn Fn(Option<&str>) -> Result<(), String> + Send + Sync>;

/// Complete [`WidgetHost`] implementation connecting `nsh/widget` to
/// [`WidgetEvaluator`], [`WidgetStore`], and the platform's reload bridge.
pub struct EngineWidgetHost {
    available: bool,
    evaluator: WidgetEvaluator,
    store: WidgetStore,
    reload_hook: ReloadHook,
}

impl EngineWidgetHost {
    /// Creates an `EngineWidgetHost`.
    pub fn new(
        available: bool,
        evaluator: WidgetEvaluator,
        store: WidgetStore,
        reload_hook: ReloadHook,
    ) -> Self {
        Self {
            available,
            evaluator,
            store,
            reload_hook,
        }
    }

    /// Returns a reference to the underlying [`WidgetEvaluator`].
    pub fn evaluator(&self) -> &WidgetEvaluator {
        &self.evaluator
    }

    /// Returns a reference to the underlying [`WidgetStore`].
    pub fn store(&self) -> &WidgetStore {
        &self.store
    }

    /// Re-evaluates all registered widgets using their last persisted data (or
    /// placeholder when no data was written yet), writes updated snapshots to
    /// the shared container, and triggers a full OS timeline reload.
    ///
    /// Called automatically when an `.nshpkg` OTA update activates!
    pub fn resync_all(&self) -> Result<(), String> {
        let now = now_ms();
        for id in self.evaluator.widget_ids() {
            let existing = self.store.read_data(&id).ok().flatten();
            let snapshot = match existing {
                Some(data) => {
                    self.evaluator
                        .evaluate_single(&id, data, WidgetReloadPolicy::AtEnd, now)?
                }
                None => self.evaluator.evaluate_placeholder(&id, now)?,
            };
            let assets = self.evaluator.assets_dir(&id);
            self.store.write_snapshot(&snapshot, assets.as_deref())?;
        }
        (self.reload_hook)(None)
    }
}

impl WidgetHost for EngineWidgetHost {
    fn available(&self) -> bool {
        self.available
    }

    fn update(
        &self,
        widget_id: &str,
        data: JsonValue,
        policy: WidgetReloadPolicy,
    ) -> Result<(), String> {
        let snapshot = self
            .evaluator
            .evaluate_single(widget_id, data.clone(), policy, now_ms())?;
        self.store.write_data(widget_id, &data)?;
        let assets = self.evaluator.assets_dir(widget_id);
        self.store.write_snapshot(&snapshot, assets.as_deref())?;
        (self.reload_hook)(Some(widget_id))
    }

    fn set_timeline(
        &self,
        widget_id: &str,
        entries: Vec<WidgetTimelineDataEntry>,
        policy: WidgetReloadPolicy,
    ) -> Result<(), String> {
        let snapshot =
            self.evaluator
                .evaluate_timeline(widget_id, &entries, policy, now_ms(), false)?;
        if let Some(first) = entries.first() {
            self.store.write_data(widget_id, &first.data)?;
        }
        let assets = self.evaluator.assets_dir(widget_id);
        self.store.write_snapshot(&snapshot, assets.as_deref())?;
        (self.reload_hook)(Some(widget_id))
    }

    fn reload(&self, widget_id: Option<&str>) -> Result<(), String> {
        (self.reload_hook)(widget_id)
    }

    fn get_data(&self, widget_id: &str) -> Result<Option<JsonValue>, String> {
        self.store.read_data(widget_id)
    }

    fn drain_pending_actions(&self) -> Vec<WidgetActionEvent> {
        self.store.drain_actions()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_widget_tsx(tsx: &str) -> String {
        nsh_bundler::compile_module("PortfolioWidget.tsx", tsx, &[nsh_base::registry().into()])
            .expect("compile widget TSX")
    }

    #[test]
    fn evaluates_multi_family_tsx_with_dual_appearance_tokens_and_controls() {
        let tsx = r#"
import { Text, View } from "nsh";
import { Link, Progress } from "nsh/base";
import { defineWidget, WidgetAction, WidgetDate } from "nsh/widget";

interface PortfolioData {
  symbol: string;
  price: string;
  changePct: string;
  goalProgress: number;
  updatedAt: number;
}

export default defineWidget<PortfolioData>({
  placeholder: {
    symbol: "AAPL",
    price: "$218.40",
    changePct: "+1.42%",
    goalProgress: 0.72,
    updatedAt: 1795280000000,
  },
  url: ({ data }) => (data ? `pulse://symbol/${data.symbol}` : "pulse://home"),
  render({ data, family }) {
    const item = data ?? {
      symbol: "AAPL",
      price: "$218.40",
      changePct: "+1.42%",
      goalProgress: 0.72,
      updatedAt: 1795280000000,
    };
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
          <Text class="text-xs font-medium text-destructive" widget={{ accentable: true }}>
            {item.changePct}
          </Text>
        </View>
        {family !== "small" && (
          <View class="flex-col gap-2">
            <Progress
              value={item.goalProgress * 100}
              class="h-1.5 rounded-full bg-muted"
              indicatorClass="h-1.5 rounded-full bg-primary"
            />
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
"#;
        let code = compile_widget_tsx(tsx);
        let theme = nsh_base::Theme::default();
        let evaluator =
            WidgetEvaluator::new(vec![nsh_base::registry()], Some(theme.tokens().clone()));
        let mut config = WidgetEntryConfig::default();
        config.id = "portfolio".into();
        config.entry = "widgets/PortfolioWidget.tsx".into();
        config.name = "Portfolio".into();
        config.families = vec![WidgetFamily::Small, WidgetFamily::Medium];
        evaluator.register(WidgetBundle::new(config, code));

        let data = serde_json::json!({
            "symbol": "NVDA",
            "price": "$142.80",
            "changePct": "+3.15%",
            "goalProgress": 0.72,
            "updatedAt": 1_795_280_000_000_i64
        });
        let snapshot = evaluator
            .evaluate_single(
                "portfolio",
                data.clone(),
                WidgetReloadPolicy::AtEnd,
                1_795_280_000_000,
            )
            .expect("evaluate widget");

        assert_eq!(snapshot.version, WIDGET_IR_VERSION);
        assert_eq!(snapshot.widget_id, "portfolio");
        assert_eq!(snapshot.entries.len(), 1);
        let entry = &snapshot.entries[0];
        assert!(entry.families.contains_key(&WidgetFamily::Small));
        assert!(entry.families.contains_key(&WidgetFamily::Medium));

        let medium = &entry.families[&WidgetFamily::Medium];
        assert_eq!(medium.url.as_deref(), Some("pulse://symbol/NVDA"));

        // Verify dual-appearance containerBackground (`bg-card` differs between Light and Dark!).
        let Some(nsh_core::widget::WidgetFill::Solid { color: bg }) = &medium.container_background
        else {
            panic!(
                "expected lifted solid container_background, got {:?}",
                medium.container_background
            );
        };
        assert_ne!(
            bg.light, bg.dark,
            "bg-card must produce distinct Light and Dark RGBA values"
        );

        // Walk IR to verify Text, Progress, Link, Action, Date were all extracted.
        let mut saw_price = false;
        let mut saw_progress = false;
        let mut saw_link = false;
        let mut saw_action = false;
        let mut saw_date = false;
        fn walk(
            node: &WidgetNode,
            saw_price: &mut bool,
            saw_progress: &mut bool,
            saw_link: &mut bool,
            saw_action: &mut bool,
            saw_date: &mut bool,
        ) {
            match node {
                WidgetNode::Box { children, .. } => {
                    for c in children {
                        walk(c, saw_price, saw_progress, saw_link, saw_action, saw_date);
                    }
                }
                WidgetNode::Text { runs, .. } => {
                    if runs.iter().any(|r| r.text.contains("$142.80")) {
                        *saw_price = true;
                        let color = runs[0].style.color.expect("text color");
                        assert_ne!(
                            color.light, color.dark,
                            "text-foreground must produce distinct Light and Dark colors"
                        );
                    }
                }
                WidgetNode::Progress { value, .. } => {
                    assert!((*value - 0.72).abs() < 0.01, "progress value = {value}");
                    *saw_progress = true;
                }
                WidgetNode::Link { href, child, .. } => {
                    assert_eq!(href, "pulse://symbol/NVDA/chart");
                    *saw_link = true;
                    walk(
                        child,
                        saw_price,
                        saw_progress,
                        saw_link,
                        saw_action,
                        saw_date,
                    );
                }
                WidgetNode::Action {
                    action,
                    payload_json,
                    child,
                    ..
                } => {
                    assert_eq!(action, "refresh");
                    assert!(
                        payload_json.as_deref().is_some_and(|p| p.contains("NVDA")),
                        "payload={payload_json:?}"
                    );
                    *saw_action = true;
                    walk(
                        child,
                        saw_price,
                        saw_progress,
                        saw_link,
                        saw_action,
                        saw_date,
                    );
                }
                WidgetNode::Date {
                    timestamp_ms, mode, ..
                } => {
                    assert_eq!(*timestamp_ms, 1_795_280_000_000);
                    assert_eq!(*mode, nsh_core::widget::WidgetDateMode::Relative);
                    *saw_date = true;
                }
                WidgetNode::Image { .. } | _ => {}
            }
        }
        walk(
            &medium.root,
            &mut saw_price,
            &mut saw_progress,
            &mut saw_link,
            &mut saw_action,
            &mut saw_date,
        );
        assert!(saw_price, "missing price text node");
        assert!(saw_progress, "missing Progress node");
        assert!(saw_link, "missing Link node");
        assert!(saw_action, "missing WidgetAction node");
        assert!(saw_date, "missing WidgetDate node");

        // Verify atomic WidgetStore persistence & action queue drain.
        let tmp = std::env::temp_dir().join(format!("nsh-widget-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let store = WidgetStore::new(&tmp);
        store.write_snapshot(&snapshot, None).unwrap();
        store.write_data("portfolio", &data).unwrap();
        assert_eq!(store.read_snapshot("portfolio").unwrap(), Some(snapshot));
        assert_eq!(store.read_data("portfolio").unwrap(), Some(data));
        let mut action_evt = WidgetActionEvent::default();
        action_evt.widget_id = "portfolio".into();
        action_evt.action = "refresh".into();
        action_evt.timestamp = 1_795_280_005_000;
        store.enqueue_action(&action_evt).unwrap();
        assert_eq!(store.drain_actions(), vec![action_evt]);
        assert!(store.drain_actions().is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }
}
