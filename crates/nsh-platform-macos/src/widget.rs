//! macOS WidgetKit host bridge: `WidgetCenter` timeline reloads, App Group
//! container resolution, manifest loading, and Darwin notification delivery for
//! interactive `<WidgetAction>` buttons.

use std::{
    ffi::{CStr, CString, c_char, c_void},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use dispatch2::DispatchQueue;
use nsh_core::{
    EventKind, EventPayload, ROOT_ID,
    element::{Registry, Tokens},
    widget::{WidgetCommand, WidgetEntryConfig, WidgetHost as _},
};
use nsh_engine::widget::{EngineWidgetHost, WidgetBundle, WidgetEvaluator, WidgetStore};
use serde::{Deserialize, Serialize};

use crate::surface::Surface;

// C ABI functions exported by `swift/NshWidgetHostBridge.swift`.
unsafe extern "C" {
    fn nsh_macos_widget_reload_kind(kind: *const c_char);
    fn nsh_macos_widget_reload_all();
    fn nsh_macos_widget_group_container(
        group_id: *const c_char,
        out_buf: *mut c_char,
        out_len: usize,
    ) -> usize;
}

// Raw CoreFoundation Darwin notification center bindings.
#[repr(C)]
struct CFNotificationCenterOpaque {
    _private: [u8; 0],
}
type CFNotificationCenterRef = *mut CFNotificationCenterOpaque;
type CFStringRef = *const c_void;
type CFNotificationCallback = unsafe extern "C" fn(
    center: CFNotificationCenterRef,
    observer: *mut c_void,
    name: CFStringRef,
    object: *const c_void,
    user_info: *const c_void,
);

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NOTIFICATION_SUSPENSION_DELIVER_IMMEDIATELY: isize = 4;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFNotificationCenterGetDarwinNotifyCenter() -> CFNotificationCenterRef;
    fn CFNotificationCenterAddObserver(
        center: CFNotificationCenterRef,
        observer: *const c_void,
        callback: CFNotificationCallback,
        name: CFStringRef,
        object: *const c_void,
        suspension_behavior: isize,
    );
    fn CFNotificationCenterRemoveObserver(
        center: CFNotificationCenterRef,
        observer: *const c_void,
        name: CFStringRef,
        object: *const c_void,
    );
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFRelease(cf: *const c_void);
}

/// Reloads timelines for one widget kind via `WidgetCenter.shared`.
pub(crate) fn reload_kind(kind: &str) {
    let Ok(cstr) = CString::new(kind) else {
        return;
    };
    // SAFETY: `cstr` is a valid NUL-terminated UTF-8 string for the duration of the call.
    unsafe { nsh_macos_widget_reload_kind(cstr.as_ptr()) };
}

/// Reloads all timelines for the app's WidgetKit extensions.
pub(crate) fn reload_all() {
    // SAFETY: FFI call with no pointer arguments.
    unsafe { nsh_macos_widget_reload_all() };
}

/// Resolves the shared App Group container directory for `app_group`, if the
/// process holds the entitlement and macOS provides a container URL.
pub(crate) fn group_container(app_group: &str) -> Option<PathBuf> {
    let cstr = CString::new(app_group).ok()?;
    let mut buf = vec![0u8; 4096];
    // SAFETY: `cstr` is NUL-terminated and `buf` has capacity `buf.len()`.
    let len = unsafe {
        nsh_macos_widget_group_container(
            cstr.as_ptr(),
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len(),
        )
    };
    if len == 0 || len >= buf.len() {
        return None;
    }
    let path = CStr::from_bytes_until_nul(&buf[..=len])
        .ok()?
        .to_str()
        .ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Handles a [`WidgetCommand`] emitted by core on the main thread.
pub(crate) fn handle_command(command: &WidgetCommand) {
    if let WidgetCommand::Reload { widget_id } = command {
        match widget_id.as_deref() {
            Some(id) => reload_kind(id),
            None => reload_all(),
        }
    }
}

/// Manifest written into `<App>.app/Contents/Resources/nsh/widgets/manifest.json`
/// (and `.nshpkg` OTA updates) describing the app's compiled widgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WidgetManifest {
    /// Shared App Group identifier (e.g. `<TEAM_ID>.<app_id>.widgets`).
    pub(crate) app_group: String,
    /// Declared widgets and their compiled JS module filenames.
    pub(crate) widgets: Vec<WidgetManifestEntry>,
}

/// One entry in [`WidgetManifest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WidgetManifestEntry {
    /// Widget configuration from `nsh.json`.
    pub(crate) config: WidgetEntryConfig,
    /// Relative JS file inside the `widgets/` directory (e.g. `"portfolio.js"`).
    pub(crate) code_file: String,
}

/// Loaded macOS widget runtime bound to one surface/engine.
pub(crate) struct LoadedWidgets {
    pub(crate) host: Arc<EngineWidgetHost>,
    pub(crate) app_group: String,
}

/// Reads only the `appGroup` identifier from `widgets_dir/manifest.json`, if present.
pub(crate) fn read_app_group(widgets_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(widgets_dir.join("manifest.json")).ok()?;
    let manifest: WidgetManifest = serde_json::from_str(&raw).ok()?;
    (!manifest.app_group.is_empty()).then_some(manifest.app_group)
}

/// Loads compiled widgets from `widgets_dir` (`<Resources>/nsh/widgets` or an
/// extracted OTA package's `widgets/` directory). When `app_group_override` is
/// supplied (e.g. the shipped bundle's signed App Group during an OTA update),
/// it takes precedence over the manifest's `appGroup`. Returns `None`
/// immediately when no `manifest.json` is present (zero overhead for apps
/// without widgets).
pub(crate) fn load_from_dir(
    widgets_dir: &Path,
    app_group_override: Option<&str>,
    ui_sets: Vec<Registry>,
    tokens: Option<Tokens>,
) -> Option<LoadedWidgets> {
    let manifest_path = widgets_dir.join("manifest.json");
    if !manifest_path.is_file() {
        return None;
    }
    let raw = match fs::read_to_string(&manifest_path) {
        Ok(raw) => raw,
        Err(err) => {
            log::warn!("widgets: failed to read {}: {err}", manifest_path.display());
            return None;
        }
    };
    let mut manifest: WidgetManifest = match serde_json::from_str(&raw) {
        Ok(manifest) => manifest,
        Err(err) => {
            log::warn!("widgets: invalid {}: {err}", manifest_path.display());
            return None;
        }
    };
    if let Some(group) = app_group_override.filter(|g| !g.is_empty()) {
        manifest.app_group = group.to_owned();
    }
    if manifest.widgets.is_empty() {
        return None;
    }

    let container = group_container(&manifest.app_group).unwrap_or_else(|| {
        let base = nsh_engine::support_dir().unwrap_or_else(std::env::temp_dir);
        base.join("nsh-widgets").join(&manifest.app_group)
    });
    let store = WidgetStore::new(container);
    let evaluator = WidgetEvaluator::new(ui_sets, tokens);
    let assets_dir = widgets_dir.join("assets");
    let assets_dir = assets_dir.is_dir().then_some(assets_dir);

    for entry in manifest.widgets {
        let code_path = widgets_dir.join(&entry.code_file);
        match fs::read_to_string(&code_path) {
            Ok(code) => {
                evaluator.register(
                    WidgetBundle::new(entry.config, code).with_assets_dir(assets_dir.clone()),
                );
            }
            Err(err) => {
                log::warn!(
                    "widgets: missing bundle {} for widget {:?}: {err}",
                    code_path.display(),
                    entry.config.id
                );
            }
        }
    }

    let reload_hook: nsh_engine::widget::ReloadHook = Arc::new(|widget_id| {
        match widget_id {
            Some(id) => reload_kind(id),
            None => reload_all(),
        }
        Ok(())
    });
    let host = Arc::new(EngineWidgetHost::new(true, evaluator, store, reload_hook));
    Some(LoadedWidgets {
        host,
        app_group: manifest.app_group,
    })
}

/// Payload passed as the opaque observer token to `CFNotificationCenter`.
struct ObserverContext {
    surface_key: u64,
    host: Arc<EngineWidgetHost>,
}

/// Active Darwin notification subscription for `<app_group>.action`, scoped to
/// one [`Surface`] and unsubscribed automatically on drop.
pub(crate) struct WidgetActionObserver {
    ctx: *mut ObserverContext,
    name: CFStringRef,
}

impl WidgetActionObserver {
    /// Subscribes to Darwin notifications for `<app_group>.action` and forwards
    /// queued [`nsh_core::widget::WidgetActionEvent`]s to `surface_key`.
    pub(crate) fn install(
        surface_key: u64,
        app_group: &str,
        host: Arc<EngineWidgetHost>,
    ) -> Option<Self> {
        let notif_name = CString::new(format!("{app_group}.action")).ok()?;
        // SAFETY: `notif_name` is a valid NUL-terminated UTF-8 string.
        let cf_name = unsafe {
            CFStringCreateWithCString(
                std::ptr::null(),
                notif_name.as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            )
        };
        if cf_name.is_null() {
            return None;
        }
        let ctx = Box::into_raw(Box::new(ObserverContext { surface_key, host }));
        // SAFETY: `center` is the process-global Darwin notify center, `ctx` stays
        // alive and pinned until `Drop` removes the observer, and `cf_name` is a
        // valid CFString retained by `WidgetActionObserver` until `Drop`.
        unsafe {
            let center = CFNotificationCenterGetDarwinNotifyCenter();
            CFNotificationCenterAddObserver(
                center,
                ctx.cast::<c_void>(),
                darwin_action_callback,
                cf_name,
                std::ptr::null(),
                K_CF_NOTIFICATION_SUSPENSION_DELIVER_IMMEDIATELY,
            );
        }
        Some(Self { ctx, name: cf_name })
    }
}

impl Drop for WidgetActionObserver {
    fn drop(&mut self) {
        // SAFETY: Removes the exact observer registered in `install` before
        // freeing `ctx` and releasing `self.name`.
        unsafe {
            let center = CFNotificationCenterGetDarwinNotifyCenter();
            CFNotificationCenterRemoveObserver(
                center,
                self.ctx.cast::<c_void>(),
                self.name,
                std::ptr::null(),
            );
            CFRelease(self.name);
            drop(Box::from_raw(self.ctx));
        }
    }
}

/// Called by CoreFoundation when a widget extension posts `<app_group>.action`.
///
/// # Safety
/// `observer` must be the live `*mut ObserverContext` registered by
/// [`WidgetActionObserver::install`].
unsafe extern "C" fn darwin_action_callback(
    _center: CFNotificationCenterRef,
    observer: *mut c_void,
    _name: CFStringRef,
    _object: *const c_void,
    _user_info: *const c_void,
) {
    if observer.is_null() {
        return;
    }
    // SAFETY: `observer` is valid for the lifetime of the registration; we
    // clone the `Arc` and copy `surface_key` before hopping to the main queue.
    let (surface_key, host) = unsafe {
        let ctx = &*(observer.cast::<ObserverContext>());
        (ctx.surface_key, ctx.host.clone())
    };
    DispatchQueue::main().exec_async(move || {
        let events = host.drain_pending_actions();
        if events.is_empty() {
            return;
        }
        if let Some(surface) = Surface::with_key(surface_key) {
            for event in events {
                if let Ok(json) = serde_json::to_value(&event) {
                    surface.event(ROOT_ID, EventKind::WidgetAction, EventPayload::Json(json));
                }
            }
        }
    });
}
