//! Automatic macOS WidgetKit `.appex` compilation, assembly, entitlements, and
//! default placeholder snapshot generation for `nsh package --target macos`.
//!
//! Apps that declare `"widgets"` in `nsh.json` get a self-contained
//! `<App>.app/Contents/PlugIns/<Executable>Widgets.appex` compiled with
//! `xcrun --sdk macosx swiftc` against `NshWidgetRuntime` (no Xcode project
//! required), plus `<App>.app/Contents/Resources/nsh/widgets/` for the main
//! app's headless `EngineWidgetHost`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail};
use nsh_core::widget::{
    WidgetEntryConfig, WidgetFamily, WidgetImageSource, WidgetNode, WidgetSnapshot,
};
use nsh_engine::widget::{WidgetBundle, WidgetEvaluator};
use serde::{Deserialize, Serialize};

use crate::{
    package::{AppConfig, Arch},
    ui,
};

/// Minimum macOS deployment target required by WidgetKit desktop + AppIntents.
const WIDGET_MIN_MACOS: &str = "14.0";

/// Manifest written into `<App>.app/Contents/Resources/nsh/widgets/manifest.json`
/// and read by `nsh-platform-macos` at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WidgetManifest {
    pub(crate) app_group: String,
    pub(crate) widgets: Vec<WidgetManifestEntry>,
}

/// One compiled widget entry in [`WidgetManifest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WidgetManifestEntry {
    pub(crate) config: WidgetEntryConfig,
    pub(crate) code_file: String,
}

/// One compiled widget module ready for packaging into `.app` or `.nshpkg`.
#[derive(Debug, Clone)]
pub(crate) struct CompiledWidget {
    pub(crate) config: WidgetEntryConfig,
    pub(crate) code: String,
    pub(crate) snapshot: WidgetSnapshot,
}

/// Result of assembling `<App>.app/Contents/PlugIns/<Executable>Widgets.appex`.
pub(crate) struct PackagedWidgets {
    pub(crate) appex_path: PathBuf,
    pub(crate) appex_entitlements: plist::Dictionary,
    pub(crate) app_group: String,
}

/// Compiles and validates every widget declared in `config.widgets`, and
/// evaluates each widget's default placeholder [`WidgetSnapshot`].
pub(crate) fn compile_widgets(
    app_dir: &Path,
    config: &AppConfig,
) -> anyhow::Result<Vec<CompiledWidget>> {
    let ui_sets_desc: Vec<_> = crate::ui_sets().to_vec();
    let theme_tokens = resolve_theme_tokens(app_dir, config);
    let evaluator = WidgetEvaluator::new(vec![nsh_base::registry()], Some(theme_tokens));

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);

    let mut compiled = Vec::with_capacity(config.widgets.len());
    for entry in &config.widgets {
        let entry_path = app_dir.join(&entry.entry);
        let diags = nsh_bundler::check_widget_file(&entry_path, entry.network)
            .map_err(|e| anyhow::anyhow!("widget {:?}: {e}", entry.id))?;
        if !diags.is_empty() {
            let mut msg = format!(
                "widget {:?} ({}) failed static validation:\n",
                entry.id, entry.entry
            );
            for d in &diags {
                msg.push_str(&format!("  {d}\n"));
            }
            bail!("{msg}");
        }
        let source = fs::read_to_string(&entry_path)
            .with_context(|| format!("read widget entry {}", entry_path.display()))?;
        let name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Widget.tsx");
        let code = nsh_bundler::compile_module(name, &source, &ui_sets_desc).map_err(|e| {
            anyhow::anyhow!("compile widget {:?} ({}):\n{e}", entry.id, entry.entry)
        })?;

        evaluator.register(WidgetBundle::new(entry.clone(), code.clone()));
        let snapshot = evaluator
            .evaluate_placeholder(&entry.id, now_ms)
            .map_err(|e| anyhow::anyhow!("evaluate placeholder for widget {:?}: {e}", entry.id))?;
        compiled.push(CompiledWidget {
            config: entry.clone(),
            code,
            snapshot,
        });
    }
    Ok(compiled)
}

fn resolve_theme_tokens(app_dir: &Path, _config: &AppConfig) -> nsh_core::element::Tokens {
    let theme_rel = fs::read_to_string(app_dir.join("nsh.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("theme")?.as_str().map(str::to_owned))
        .filter(|s| !s.is_empty());
    if let Some(theme_rel) = theme_rel {
        let path = app_dir.join(theme_rel);
        if let Ok(json) = fs::read_to_string(&path)
            && let Ok(override_tokens) = nsh_base::theme_overrides(&json)
        {
            return override_tokens.over(nsh_base::Theme::default().tokens());
        }
    }
    nsh_base::Theme::default().tokens().clone()
}

/// Resolves the macOS App Group identifier shared between main `.app` and `.appex`.
///
/// - Uses explicit `macos.widgets.appGroup` if configured.
/// - Otherwise, when signed with a certificate identity (`identity != "-"`),
///   extracts the Apple Team ID (`OU=...`) to form `<TEAM_ID>.<app_id>.widgets`
///   (required for Developer ID + DMG distribution without a provisioning profile).
/// - Otherwise (ad-hoc `-`), falls back to `nsh.local.<app_id>.widgets`.
pub(crate) fn resolve_app_group(
    app_id: &str,
    config: &AppConfig,
    identity: &str,
) -> (String, bool) {
    if let Some(group) = config
        .macos
        .widgets
        .app_group
        .as_deref()
        .filter(|g| !g.trim().is_empty())
    {
        return (group.trim().to_owned(), false);
    }
    if identity != "-"
        && let Some(team_id) = extract_team_id(identity)
    {
        return (format!("{team_id}.{app_id}.widgets"), false);
    }
    (format!("nsh.local.{app_id}.widgets"), true)
}

/// Extracts the 10-character Apple Team ID (`OU=...`) from a keychain signing
/// identity, if present.
fn extract_team_id(identity: &str) -> Option<String> {
    let pem = Command::new("security")
        .args(["find-certificate", "-c", identity, "-p"])
        .output()
        .ok()?;
    if !pem.status.success() || pem.stdout.is_empty() {
        return None;
    }
    let mut openssl = Command::new("openssl")
        .args(["x509", "-noout", "-subject"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    use std::io::Write as _;
    openssl.stdin.take()?.write_all(&pem.stdout).ok()?;
    let out = openssl.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let subject = String::from_utf8_lossy(&out.stdout);
    parse_team_id_from_subject(&subject)
}

fn parse_team_id_from_subject(subject: &str) -> Option<String> {
    for part in subject.split([',', '/']) {
        let trimmed = part.trim();
        if let Some(val) = trimmed
            .strip_prefix("OU=")
            .or_else(|| trimmed.strip_prefix("OU = "))
        {
            let ou = val.trim();
            if ou.len() == 10 && ou.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(ou.to_owned());
            }
        }
    }
    None
}

/// Builds and embeds `<App>.app/Contents/PlugIns/<Executable>Widgets.appex` and
/// writes the main app's `<App>.app/Contents/Resources/nsh/widgets/` payload.
#[allow(
    clippy::too_many_arguments,
    reason = "packaging step receives app metadata, bundle paths, and signing identity"
)]
pub(crate) fn package_macos_widgets(
    app_dir: &Path,
    app_bundle: &Path,
    nsh_resources: &Path,
    app_id: &str,
    executable: &str,
    main_info: &plist::Dictionary,
    config: &AppConfig,
    built_assets: &[crate::build::Asset],
    arch: Option<Arch>,
    identity: &str,
) -> anyhow::Result<PackagedWidgets> {
    let step = ui::Step::start(format!("building {} macOS widget(s)", config.widgets.len()));
    let compiled = compile_widgets(app_dir, config)?;
    let (app_group, is_local_fallback) = resolve_app_group(app_id, config, identity);
    if is_local_fallback {
        ui::warn(format!(
            "widgets: using ad-hoc App Group `{app_group}`; pass --sign <Developer ID> (or set macos.widgets.appGroup) for real macOS App Group container sharing"
        ));
    }

    // 1. Write main app resources: Contents/Resources/nsh/widgets/{manifest.json, <id>.js, assets/}
    let main_widgets_dir = nsh_resources.join("widgets");
    fs::create_dir_all(&main_widgets_dir)?;
    let mut manifest_entries = Vec::with_capacity(compiled.len());
    let mut default_snapshots = BTreeMap::new();
    let mut referenced_assets = BTreeSet::new();

    for w in &compiled {
        let code_file = format!("{}.js", w.config.id);
        fs::write(main_widgets_dir.join(&code_file), &w.code)?;
        collect_snapshot_assets(&w.snapshot, &mut referenced_assets);
        default_snapshots.insert(w.config.id.clone(), w.snapshot.clone());
        manifest_entries.push(WidgetManifestEntry {
            config: w.config.clone(),
            code_file,
        });
    }
    let manifest = WidgetManifest {
        app_group: app_group.clone(),
        widgets: manifest_entries,
    };
    fs::write(
        main_widgets_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    // 2. Generate tiny per-app Swift entry point (.nsh/build/macos-widgets/WidgetsMain.swift)
    let build_dir = app_dir.join(".nsh/build/macos-widgets");
    fs::create_dir_all(&build_dir)?;
    let swift_main = build_dir.join("WidgetsMain.swift");
    let swift_source = generate_widgets_main_swift(&config.widgets, &app_group);
    fs::write(&swift_main, swift_source)?;

    // 3. Compile Swift extension executable for target architecture(s)
    let ext_exec_name = format!("{executable}Widgets");
    let target_archs: Vec<&str> = match arch {
        Some(Arch::Arm64) => vec!["arm64"],
        Some(Arch::X86_64) => vec!["x86_64"],
        Some(Arch::Universal) => vec!["arm64", "x86_64"],
        None => vec![if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        }],
    };
    let mut arch_binaries = Vec::with_capacity(target_archs.len());
    for target_arch in &target_archs {
        let runtime_dir = crate::runtime::macos_widget_runtime(target_arch)?;
        let out_bin = build_dir.join(format!("{ext_exec_name}-{target_arch}"));
        let cache = build_dir.join("module-cache");
        fs::create_dir_all(&cache)?;
        let output = Command::new("xcrun")
            .args(["--sdk", "macosx", "swiftc"])
            .arg("-module-cache-path")
            .arg(&cache)
            .args([
                "-parse-as-library",
                "-target",
                &format!("{target_arch}-apple-macosx{WIDGET_MIN_MACOS}"),
                "-swift-version",
                "5",
                "-O",
                "-application-extension",
            ])
            .arg("-I")
            .arg(&runtime_dir)
            .arg("-L")
            .arg(&runtime_dir)
            .args([
                "-lNshWidgetRuntime",
                "-framework",
                "WidgetKit",
                "-framework",
                "SwiftUI",
                "-framework",
                "AppIntents",
            ])
            .arg("-o")
            .arg(&out_bin)
            .arg(&swift_main)
            .output()
            .context("run xcrun --sdk macosx swiftc for widget extension")?;
        if !output.status.success() {
            bail!(
                "compiling macOS widget extension ({target_arch}) failed:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        arch_binaries.push(out_bin);
    }

    // 4. Assemble <App>.app/Contents/PlugIns/<Executable>Widgets.appex
    let appex_path = app_bundle
        .join("Contents/PlugIns")
        .join(format!("{ext_exec_name}.appex"));
    let ext_contents = appex_path.join("Contents");
    let ext_macos = ext_contents.join("MacOS");
    let ext_widgets_res = ext_contents.join("Resources/nsh/widgets");
    fs::create_dir_all(&ext_macos)?;
    fs::create_dir_all(&ext_widgets_res)?;

    let ext_exec_path = ext_macos.join(&ext_exec_name);
    if arch_binaries.len() == 1 {
        fs::copy(&arch_binaries[0], &ext_exec_path)?;
    } else {
        let mut cmd = Command::new("lipo");
        cmd.arg("-create");
        for bin in &arch_binaries {
            cmd.arg(bin);
        }
        cmd.arg("-output").arg(&ext_exec_path);
        let out = cmd
            .output()
            .context("run lipo for universal widget extension")?;
        if !out.status.success() {
            bail!(
                "lipo widget extension failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(&ext_exec_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&ext_exec_path, perms)?;
    }

    // Write default placeholder snapshots & copy referenced assets into both
    // the extension and main app's widget resources.
    let snapshots_json = serde_json::to_vec(&default_snapshots)?;
    fs::write(
        ext_widgets_res.join("default-snapshots.json"),
        &snapshots_json,
    )?;
    copy_widget_assets(
        &referenced_assets,
        built_assets,
        &ext_widgets_res.join("assets"),
        &main_widgets_dir.join("assets"),
    )?;

    // Write .appex Contents/Info.plist
    let ext_info = widget_extension_info_plist(app_id, &ext_exec_name, main_info);
    plist::Value::Dictionary(ext_info)
        .to_file_xml(ext_contents.join("Info.plist"))
        .context("write widget extension Info.plist")?;

    let needs_network = config.widgets.iter().any(|w| w.network);
    let appex_entitlements = widget_extension_entitlements(&app_group, needs_network);

    step.done(format!("assembled {}", ui::path(&appex_path)));
    Ok(PackagedWidgets {
        appex_path,
        appex_entitlements,
        app_group,
    })
}

/// Merges `app_group` into the main `.app`'s `com.apple.security.application-groups`
/// entitlements array without clobbering any existing groups.
pub(crate) fn merge_main_app_group(entitlements: &mut plist::Dictionary, app_group: &str) {
    let key = "com.apple.security.application-groups";
    let mut arr = match entitlements.get(key).and_then(plist::Value::as_array) {
        Some(existing) => existing.clone(),
        None => Vec::new(),
    };
    if !arr
        .iter()
        .any(|v| v.as_string().is_some_and(|s| s == app_group))
    {
        arr.push(plist::Value::String(app_group.to_owned()));
    }
    entitlements.insert(key.to_owned(), plist::Value::Array(arr));
}

/// Builds the mandatory macOS WidgetKit extension entitlements plist.
pub(crate) fn widget_extension_entitlements(app_group: &str, network: bool) -> plist::Dictionary {
    let mut d = plist::Dictionary::new();
    // macOS `chronod` silently refuses to load non-sandboxed WidgetKit extensions.
    d.insert("com.apple.security.app-sandbox".into(), true.into());
    d.insert(
        "com.apple.security.application-groups".into(),
        plist::Value::Array(vec![plist::Value::String(app_group.to_owned())]),
    );
    if network {
        d.insert("com.apple.security.network.client".into(), true.into());
    }
    d
}

/// Builds `Contents/Info.plist` for `<Executable>Widgets.appex`.
pub(crate) fn widget_extension_info_plist(
    app_id: &str,
    executable: &str,
    main_info: &plist::Dictionary,
) -> plist::Dictionary {
    use plist::Value;
    let s = |v: &str| Value::String(v.to_owned());
    let version = main_info
        .get("CFBundleShortVersionString")
        .and_then(Value::as_string)
        .unwrap_or("1.0.0");
    let build = main_info
        .get("CFBundleVersion")
        .and_then(Value::as_string)
        .unwrap_or("1");
    let min_sys = main_info
        .get("LSMinimumSystemVersion")
        .and_then(Value::as_string)
        .filter(|v| version_at_least(v, WIDGET_MIN_MACOS))
        .unwrap_or(WIDGET_MIN_MACOS);

    let mut ext = plist::Dictionary::new();
    ext.insert(
        "NSExtensionPointIdentifier".into(),
        s("com.apple.widgetkit-extension"),
    );

    let mut d = plist::Dictionary::new();
    d.insert("CFBundleDevelopmentRegion".into(), s("en"));
    d.insert("CFBundleDisplayName".into(), s(executable));
    d.insert("CFBundleExecutable".into(), s(executable));
    d.insert("CFBundleIdentifier".into(), s(&format!("{app_id}.widgets")));
    d.insert("CFBundleInfoDictionaryVersion".into(), s("6.0"));
    d.insert("CFBundleName".into(), s(executable));
    d.insert("CFBundlePackageType".into(), s("XPC!"));
    d.insert("CFBundleShortVersionString".into(), s(version));
    d.insert("CFBundleVersion".into(), s(build));
    d.insert(
        "CFBundleSupportedPlatforms".into(),
        Value::Array(vec![s("MacOSX")]),
    );
    d.insert("LSMinimumSystemVersion".into(), s(min_sys));
    d.insert("NSExtension".into(), Value::Dictionary(ext));
    d
}

fn version_at_least(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.')
            .map(|p| p.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let len = va.len().max(vb.len());
    for i in 0..len {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    true
}

/// Generates `.nsh/build/macos-widgets/WidgetsMain.swift` declaring one SwiftUI
/// `Widget` per entry in `widgets` and the `@main` `WidgetBundle`.
pub(crate) fn generate_widgets_main_swift(
    widgets: &[WidgetEntryConfig],
    app_group: &str,
) -> String {
    let mut out = String::from(
        "// Generated by Native Shell (`nsh package --target macos`). Do not edit.\n\
         import SwiftUI\n\
         import WidgetKit\n\
         import NshWidgetRuntime\n\n",
    );
    let mut struct_names = Vec::with_capacity(widgets.len());
    for (idx, w) in widgets.iter().enumerate() {
        let name = format!("NshWidget_{}_{}", idx, sanitize_swift_ident(&w.id));
        let families: Vec<&str> = w
            .families
            .iter()
            .map(|f| match f {
                WidgetFamily::Small => ".systemSmall",
                WidgetFamily::Medium => ".systemMedium",
                WidgetFamily::Large => ".systemLarge",
                _ => ".systemMedium",
            })
            .collect();
        let desc = w.description.as_str();
        out.push_str(&format!(
            "struct {name}: Widget {{\n\
             \x20   var body: some WidgetConfiguration {{\n\
             \x20       StaticConfiguration(\n\
             \x20           kind: {id},\n\
             \x20           provider: NshTimelineProvider(widgetId: {id}, appGroup: {group})\n\
             \x20       ) {{ entry in\n\
             \x20           NshWidgetRootView(entry: entry)\n\
             \x20       }}\n\
             \x20       .configurationDisplayName({display_name})\n\
             \x20       .description({description})\n\
             \x20       .supportedFamilies([{families}])\n\
             \x20   }}\n\
             }}\n\n",
            id = swift_string(&w.id),
            group = swift_string(app_group),
            display_name = swift_string(&w.name),
            description = swift_string(desc),
            families = families.join(", "),
        ));
        struct_names.push(name);
    }

    out.push_str(
        "@main\nstruct NshAppWidgetsBundle: WidgetBundle {\n    var body: some Widget {\n",
    );
    for name in &struct_names {
        out.push_str(&format!("        {name}()\n"));
    }
    out.push_str("    }\n}\n");
    out
}

fn sanitize_swift_ident(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("Item");
    }
    out
}

fn swift_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
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
            for c in children {
                collect_node_assets(c, out);
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
        _ => {}
    }
}

fn copy_widget_assets(
    referenced: &BTreeSet<String>,
    built_assets: &[crate::build::Asset],
    ext_assets_dir: &Path,
    main_assets_dir: &Path,
) -> anyhow::Result<()> {
    for rel in referenced {
        if let Some(asset) = built_assets.iter().find(|a| a.path == *rel) {
            for dir in [ext_assets_dir, main_assets_dir] {
                let dst = dir.join(rel);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&asset.source, &dst)
                    .with_context(|| format!("copy widget asset {rel}"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_widgets_main_swift_and_info_plist() {
        let mut entry = WidgetEntryConfig::default();
        entry.id = "portfolio".into();
        entry.entry = "widgets/PortfolioWidget.tsx".into();
        entry.name = "Portfolio".into();
        entry.description = "Watchlist \"live\" quotes".into();
        entry.families = vec![WidgetFamily::Small, WidgetFamily::Medium];

        let swift = generate_widgets_main_swift(&[entry], "ABCDE12345.com.example.pulse.widgets");
        assert!(swift.contains("import NshWidgetRuntime"));
        assert!(swift.contains("kind: \"portfolio\""));
        assert!(swift.contains("appGroup: \"ABCDE12345.com.example.pulse.widgets\""));
        assert!(swift.contains(".configurationDisplayName(\"Portfolio\")"));
        assert!(swift.contains(".description(\"Watchlist \\\"live\\\" quotes\")"));
        assert!(swift.contains(".supportedFamilies([.systemSmall, .systemMedium])"));
        assert!(swift.contains("@main\nstruct NshAppWidgetsBundle: WidgetBundle"));

        let mut main_info = plist::Dictionary::new();
        main_info.insert("CFBundleShortVersionString".into(), "2.4.0".into());
        main_info.insert("CFBundleVersion".into(), "42".into());
        main_info.insert("LSMinimumSystemVersion".into(), "13.0".into());

        let ext_info = widget_extension_info_plist("com.example.pulse", "PulseWidgets", &main_info);
        assert_eq!(
            ext_info
                .get("CFBundleIdentifier")
                .and_then(plist::Value::as_string),
            Some("com.example.pulse.widgets")
        );
        assert_eq!(
            ext_info
                .get("CFBundlePackageType")
                .and_then(plist::Value::as_string),
            Some("XPC!")
        );
        assert_eq!(
            ext_info
                .get("CFBundleShortVersionString")
                .and_then(plist::Value::as_string),
            Some("2.4.0")
        );
        assert_eq!(
            ext_info
                .get("LSMinimumSystemVersion")
                .and_then(plist::Value::as_string),
            Some("14.0")
        );
    }

    #[test]
    fn derives_team_id_and_merges_entitlements() {
        assert_eq!(
            parse_team_id_from_subject(
                "subject=UID=ABCDE12345, CN=Developer ID Application: Example Inc (ABCDE12345), OU=ABCDE12345, O=Example Inc, C=US"
            ).as_deref(),
            Some("ABCDE12345")
        );

        let ext_ent = widget_extension_entitlements("ABCDE12345.com.example.app.widgets", true);
        assert_eq!(
            ext_ent
                .get("com.apple.security.app-sandbox")
                .and_then(plist::Value::as_boolean),
            Some(true)
        );
        assert_eq!(
            ext_ent
                .get("com.apple.security.network.client")
                .and_then(plist::Value::as_boolean),
            Some(true)
        );

        let mut main_ent = plist::Dictionary::new();
        merge_main_app_group(&mut main_ent, "ABCDE12345.com.example.app.widgets");
        merge_main_app_group(&mut main_ent, "ABCDE12345.com.example.app.widgets");
        let arr = main_ent
            .get("com.apple.security.application-groups")
            .and_then(plist::Value::as_array)
            .unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0].as_string(),
            Some("ABCDE12345.com.example.app.widgets")
        );
    }

    /// Cross-language golden fixture test: serializes a full Rust `WidgetSnapshot`
    /// containing every node kind (`Box`, `Text`, `Date`, `Image`, `Progress`,
    /// `Link`, `Action`, `LinearGradient`, dual Light/Dark `WidgetColor`), then
    /// compiles and executes Swift's `WidgetIR.swift` to verify zero-loss JSON
    /// decoding across the Rust ↔ Swift wire boundary.
    #[test]
    #[cfg(target_os = "macos")]
    fn rust_widget_snapshot_decodes_cleanly_in_swift() {
        use nsh_core::{
            Points,
            widget::{
                WIDGET_IR_VERSION, WidgetAccentedImageMode, WidgetAxis, WidgetBoxStyle,
                WidgetColor, WidgetCrossAlign, WidgetDateMode, WidgetEdges, WidgetFamilyTree,
                WidgetFill, WidgetGradientStop, WidgetLayout, WidgetMainAlign, WidgetResizeMode,
                WidgetTextAlign, WidgetTextRun, WidgetTextStyle, WidgetTimelineIrEntry,
            },
        };

        let mut families = BTreeMap::new();
        let mut tree = WidgetFamilyTree::default();
        tree.url = Some("pulse://symbol/AAPL".into());
        let mut stop0 = WidgetGradientStop::default();
        stop0.color = WidgetColor {
            light: [1.0, 1.0, 1.0, 1.0],
            dark: [0.08, 0.08, 0.1, 1.0],
        };
        stop0.position = 0.0;
        let mut stop1 = WidgetGradientStop::default();
        stop1.color = WidgetColor {
            light: [0.94, 0.96, 1.0, 1.0],
            dark: [0.12, 0.14, 0.2, 1.0],
        };
        stop1.position = 1.0;
        tree.container_background = Some(WidgetFill::LinearGradient {
            angle_deg: 135.0,
            stops: vec![stop0, stop1],
        });
        let mut root_layout = WidgetLayout::default();
        root_layout.axis = WidgetAxis::Column;
        root_layout.main_align = WidgetMainAlign::SpaceBetween;
        root_layout.cross_align = WidgetCrossAlign::Stretch;
        root_layout.gap = Points(6.0);
        root_layout.padding = WidgetEdges {
            top: Points(12.0),
            right: Points(12.0),
            bottom: Points(12.0),
            left: Points(12.0),
        };
        root_layout.flex_grow = 1.0;

        let mut title_style = WidgetTextStyle::default();
        title_style.font_size = Points(14.0);
        title_style.font_weight = 600;
        title_style.monospaced = true;
        title_style.color = Some(WidgetColor {
            light: [0.1, 0.1, 0.1, 1.0],
            dark: [0.95, 0.95, 0.98, 1.0],
        });
        title_style.letter_spacing = Some(Points(0.5));
        let mut title_run = WidgetTextRun::default();
        title_run.text = "AAPL".into();
        title_run.style = title_style;

        let mut btn_run = WidgetTextRun::default();
        btn_run.text = "Refresh".into();

        tree.root = WidgetNode::Box {
            layout: root_layout,
            style: WidgetBoxStyle::default(),
            children: vec![
                WidgetNode::Text {
                    runs: vec![title_run],
                    align: WidgetTextAlign::Leading,
                    line_limit: Some(1),
                    accentable: true,
                    layout: WidgetLayout::default(),
                },
                WidgetNode::Image {
                    source: WidgetImageSource::Symbol {
                        name: "chart.line.uptrend.xyaxis".into(),
                    },
                    resize_mode: WidgetResizeMode::Contain,
                    tint: Some(WidgetColor {
                        light: [0.0, 0.6, 0.4, 1.0],
                        dark: [0.2, 0.9, 0.6, 1.0],
                    }),
                    accented_mode: WidgetAccentedImageMode::Desaturated,
                    style: WidgetBoxStyle::default(),
                    layout: WidgetLayout::default(),
                },
                WidgetNode::Progress {
                    value: 0.72,
                    track_color: Some(WidgetColor {
                        light: [0.9, 0.9, 0.92, 1.0],
                        dark: [0.2, 0.2, 0.24, 1.0],
                    }),
                    indicator_color: Some(WidgetColor {
                        light: [0.06, 0.72, 0.5, 1.0],
                        dark: [0.2, 0.85, 0.6, 1.0],
                    }),
                    corner_radius: Some(Points(3.0)),
                    layout: WidgetLayout::default(),
                },
                WidgetNode::Link {
                    href: "pulse://symbol/AAPL/chart".into(),
                    accessibility_label: Some("Open AAPL chart".into()),
                    child: Box::new(WidgetNode::Date {
                        timestamp_ms: 1_795_280_000_000,
                        mode: WidgetDateMode::Relative,
                        style: WidgetTextStyle::default(),
                        accentable: false,
                        layout: WidgetLayout::default(),
                    }),
                },
                WidgetNode::Action {
                    action: "refresh".into(),
                    payload_json: Some(r#"{"symbol":"AAPL"}"#.into()),
                    patch_json: Some(r#"{"stale":false}"#.into()),
                    accessibility_label: Some("Refresh".into()),
                    child: Box::new(WidgetNode::Text {
                        runs: vec![btn_run],
                        align: WidgetTextAlign::Center,
                        line_limit: None,
                        accentable: false,
                        layout: WidgetLayout::default(),
                    }),
                },
            ],
        };
        families.insert(WidgetFamily::Medium, tree);

        let mut entry = WidgetTimelineIrEntry::default();
        entry.date_ms = 1_795_280_000_000;
        entry.relevance = Some(0.85);
        entry.families = families;

        let mut snapshot = WidgetSnapshot::default();
        snapshot.version = WIDGET_IR_VERSION;
        snapshot.widget_id = "portfolio".into();
        snapshot.generated_at_ms = 1_795_280_000_000;
        snapshot.reload_after_ms = Some(900_000);
        snapshot.entries = vec![entry];

        let json = serde_json::to_string(&snapshot).unwrap();
        let dir = std::env::temp_dir().join(format!("nsh-swift-ir-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("golden.json");
        fs::write(&json_path, &json).unwrap();

        let swift_main = dir.join("main.swift");
        fs::write(
            &swift_main,
            r#"
import Foundation

let url = URL(fileURLWithPath: CommandLine.arguments[1])
let data = try Data(contentsOf: url)
let snapshot = try JSONDecoder().decode(WidgetSnapshot.self, from: data)
precondition(snapshot.version == 1)
precondition(snapshot.widgetId == "portfolio")
precondition(snapshot.reloadAfterMs == 900_000)
precondition(snapshot.entries.count == 1)
let tree = snapshot.entries[0].families["medium"]!
precondition(tree.url == "pulse://symbol/AAPL")
if case .linearGradient(let deg, let stops) = tree.containerBackground {
    precondition(abs(deg - 135.0) < 0.01 && stops.count == 2)
} else {
    fatalError("expected linearGradient containerBackground")
}
if case .box(let layout, _, let children) = tree.root {
    precondition(layout.axis == .column && layout.mainAlign == .spaceBetween)
    precondition(children.count == 5)
    if case .progress(let v, _, _, let cr, _) = children[2] {
        precondition(abs(v - 0.72) < 0.01 && cr == 3.0)
    } else { fatalError("expected progress") }
    if case .link(let href, let label, .date(let ts, let mode, _, _, _)) = children[3] {
        precondition(href == "pulse://symbol/AAPL/chart" && label == "Open AAPL chart" && ts == 1_795_280_000_000 && mode == .relative)
    } else { fatalError("expected link->date") }
    if case .action(let act, let payload, let patch, _, _) = children[4] {
        precondition(act == "refresh" && payload?.contains("AAPL") == true && patch?.contains("stale") == true)
    } else { fatalError("expected action") }
} else {
    fatalError("expected root box")
}
let reencoded = try JSONEncoder().encode(snapshot)
let roundtrip = try JSONDecoder().decode(WidgetSnapshot.self, from: reencoded)
precondition(roundtrip == snapshot)
print("SWIFT_IR_OK")
"#,
        )
        .unwrap();

        let ir_swift = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../nsh-platform-macos/swift/widget/WidgetIR.swift");
        let bin = dir.join("verify_ir");
        let cache = dir.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let compile = Command::new("xcrun")
            .args(["--sdk", "macosx", "swiftc"])
            .arg("-module-cache-path")
            .arg(&cache)
            .args(["-swift-version", "5", "-o"])
            .arg(&bin)
            .arg(&ir_swift)
            .arg(&swift_main)
            .output()
            .expect("run swiftc");
        assert!(
            compile.status.success(),
            "swiftc failed:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&bin)
            .arg(&json_path)
            .output()
            .expect("run verify_ir");
        assert!(
            run.status.success(),
            "verify_ir failed:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(String::from_utf8_lossy(&run.stdout).contains("SWIFT_IR_OK"));
        let _ = fs::remove_dir_all(&dir);
    }
}
