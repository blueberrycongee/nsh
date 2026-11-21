//! Platform-neutral desktop and mobile widget support (`nsh/widget`).
//!
//! This module defines:
//! - [`config`]: declarative `nsh.json` `"widgets"` configuration & validation.
//! - [`ir`]: [`WidgetSnapshot`] / [`WidgetNode`] platform-neutral Widget Tree IR v1.
//! - [`components`]: [`WidgetAction`] and [`WidgetDate`] Rust components.
//! - [`WidgetHost`]: platform/engine bridge implemented by `nsh-engine` and macOS.

pub mod components;
pub mod config;
pub mod ir;

pub use components::{
    WidgetAction, WidgetActionMeta, WidgetActionProps, WidgetDate, WidgetDateMeta, WidgetDateProps,
    WidgetNodeMeta,
};
pub use config::{
    MIN_REFRESH_INTERVAL_SECS, MacosWidgetsConfig, WidgetEntryConfig, WidgetFamily,
    is_valid_widget_id, parse_refresh_interval, validate_widgets,
};
pub use ir::{
    MAX_SNAPSHOT_BYTES, MAX_WIDGET_NODES, MAX_WIDGET_TREE_DEPTH, WIDGET_IR_VERSION,
    WidgetAccentedImageMode, WidgetAxis, WidgetBoxStyle, WidgetBudgetError, WidgetColor,
    WidgetCrossAlign, WidgetDateMode, WidgetEdges, WidgetFamilyTree, WidgetFill,
    WidgetGradientStop, WidgetHints, WidgetImageSource, WidgetLayout, WidgetMainAlign, WidgetNode,
    WidgetResizeMode, WidgetSnapshot, WidgetTextAlign, WidgetTextRun, WidgetTextStyle,
    WidgetTimelineIrEntry,
};

use crate::JsonValue;

/// Policy controlling when the OS should next request a widget timeline refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum WidgetReloadPolicy {
    /// Ask the OS to refresh after the last timeline entry is reached.
    #[default]
    AtEnd,
    /// Do not schedule automatic refreshes until the app explicitly calls `Widget.reload`.
    Never,
    /// Ask the OS to refresh after the given duration in milliseconds.
    After(i64),
}

/// One data entry passed to `Widget.setTimeline`.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct WidgetTimelineDataEntry {
    /// Unix timestamp in milliseconds when this entry becomes active.
    pub date_ms: i64,
    /// JSON-serializable widget state passed to the widget component.
    pub data: JsonValue,
    /// Optional relevance score (`0.0..=1.0`) for Smart Stack ranking.
    pub relevance: Option<f32>,
}

/// Interactive action triggered from a widget button (`<WidgetAction>`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WidgetActionEvent {
    /// Widget id from `nsh.json`.
    pub widget_id: String,
    /// Action identifier from `<WidgetAction action="..."/>`.
    pub action: String,
    /// Optional JSON payload attached to the button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonValue>,
    /// Unix timestamp in milliseconds when the user clicked the action.
    pub timestamp: i64,
}

/// Maximum number of widget action events buffered while no JS listener is attached.
pub const MAX_PENDING_ACTIONS: usize = 32;

/// Command sent from [`crate::Core`] to the platform backend for widgets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WidgetCommand {
    /// Ask the OS widget host (e.g. macOS `WidgetCenter`) to reload one widget
    /// kind or all widget kinds (`None`).
    Reload {
        /// Specific widget id, or `None` for all widgets.
        widget_id: Option<String>,
    },
    /// Start delivering widget action events (and drain any queued offline actions).
    ListenActions,
}

/// Host implementation backing `nsh/widget` calls on the engine thread / background.
///
/// Provided by `nsh-engine` when widgets are configured and supported on the
/// current platform; `None` on platforms without widget support (in which case
/// `Widget.available()` resolves to `false`).
pub trait WidgetHost: Send + Sync + 'static {
    /// Whether this platform and build supports native widgets.
    fn available(&self) -> bool;

    /// Evaluates the widget TSX with `data`, persists the snapshot atomically,
    /// and notifies the OS widget host to reload.
    fn update(
        &self,
        widget_id: &str,
        data: JsonValue,
        policy: WidgetReloadPolicy,
    ) -> Result<(), String>;

    /// Evaluates a multi-entry scheduled timeline, persists it atomically, and
    /// notifies the OS widget host to reload.
    fn set_timeline(
        &self,
        widget_id: &str,
        entries: Vec<WidgetTimelineDataEntry>,
        policy: WidgetReloadPolicy,
    ) -> Result<(), String>;

    /// Asks the OS widget host to reload `widget_id` (or all widgets if `None`).
    fn reload(&self, widget_id: Option<&str>) -> Result<(), String>;

    /// Reads the last persisted data for `widget_id` (`None` if never written).
    fn get_data(&self, widget_id: &str) -> Result<Option<JsonValue>, String>;

    /// Drains any pending interactive widget actions persisted while the app
    /// was closed or before a listener attached.
    fn drain_pending_actions(&self) -> Vec<WidgetActionEvent> {
        Vec::new()
    }
}
