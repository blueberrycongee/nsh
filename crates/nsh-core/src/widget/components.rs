//! Widget-specific Rust components exported from `nsh/widget`:
//! [`WidgetAction`] (interactive desktop button) and [`WidgetDate`] (live
//! OS-driven date/countdown text).

use super::ir::{WidgetDateMode, WidgetHints};
use crate::{JsonValue, prelude::*};

/// Internal widget metadata attached to a host node in [`crate::Core`] without
/// growing `Node` for non-widget views.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WidgetNodeMeta {
    /// Visual hints from `widget={{ ... }}`.
    pub hints: Option<WidgetHints>,
    /// Interactive action metadata from `<WidgetAction>`.
    pub action: Option<WidgetActionMeta>,
    /// Live OS-driven date metadata from `<WidgetDate>`.
    pub date: Option<WidgetDateMeta>,
}

/// Interactive button metadata attached by [`WidgetAction`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WidgetActionMeta {
    /// Action identifier.
    pub action: String,
    /// Optional JSON-serialized payload.
    pub payload_json: Option<String>,
    /// Optional JSON-serialized optimistic offline state patch.
    pub patch_json: Option<String>,
    /// Optional VoiceOver / accessibility label.
    pub accessibility_label: Option<String>,
}

/// Live date metadata attached by [`WidgetDate`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WidgetDateMeta {
    /// Target Unix timestamp in milliseconds.
    pub timestamp_ms: i64,
    /// Display mode (`"relative"`, `"timer"`, `"offset"`, `"time"`, `"date"`).
    pub mode: WidgetDateMode,
}

/// Props of [`WidgetAction`].
#[derive(Debug, Clone, PartialEq, Props)]
pub struct WidgetActionProps {
    /// Action name dispatched to `Widget.on("action", ...)` and `onAction`.
    pub action: Prop<SharedString>,
    /// Optional JSON-serializable payload attached to the action button.
    pub payload: Prop<Option<JsonValue>>,
    /// Optional optimistic state patch applied natively by the extension when
    /// the main app is closed.
    pub patch: Prop<Option<JsonValue>>,
    /// Accessible button label for assistive technologies (VoiceOver).
    pub accessibility_label: Prop<Option<SharedString>>,
    /// Visual content of the interactive button.
    pub children: Prop<Children>,
}

/// Interactive desktop button inside a widget.
///
/// Maps to SwiftUI `Button(intent:)` (macOS 14+ `AppIntents`) on Apple
/// platforms and `Action.Execute` on Windows Adaptive Cards.
#[component]
pub fn WidgetAction(props: WidgetActionProps, _cx: Scope<NoEvent>) -> impl IntoElement {
    let action = props.action;
    let payload = props.payload;
    let patch = props.patch;
    let label = props.accessibility_label;
    view()
        .accessible(true)
        .role(AccessibilityRole::Button)
        .optional_label(label)
        .__widget_meta(Prop::bind(move || {
            let payload_json = payload
                .get()
                .and_then(|v| (!v.is_null()).then(|| v.to_string()));
            let patch_json = patch
                .get()
                .and_then(|v| (!v.is_null()).then(|| v.to_string()));
            Some(WidgetNodeMeta {
                action: Some(WidgetActionMeta {
                    action: action.get().to_string(),
                    payload_json,
                    patch_json,
                    accessibility_label: label.get().map(|s| s.to_string()),
                }),
                ..WidgetNodeMeta::default()
            })
        }))
        .child(props.children)
}

/// Props of [`WidgetDate`].
#[derive(Debug, Clone, PartialEq, Props)]
pub struct WidgetDateProps {
    /// Unix timestamp in milliseconds.
    pub date: Prop<f64>,
    /// Live date formatting mode (default `"relative"`).
    #[prop(default)]
    pub mode: Prop<WidgetDateMode>,
}

/// OS-driven live date/countdown text that ticks natively inside WidgetKit
/// without waking JS or consuming timeline budget.
#[component]
pub fn WidgetDate(props: WidgetDateProps, _cx: Scope<NoEvent>) -> impl IntoElement {
    let date = props.date;
    let mode = props.mode;
    text(move || format_preview_date(date.get() as i64, mode.get())).__widget_meta(Prop::bind(
        move || {
            Some(WidgetNodeMeta {
                date: Some(WidgetDateMeta {
                    timestamp_ms: date.get() as i64,
                    mode: mode.get(),
                }),
                ..WidgetNodeMeta::default()
            })
        },
    ))
}

/// Fallback text rendered by [`WidgetDate`] in non-WidgetKit preview surfaces.
fn format_preview_date(timestamp_ms: i64, mode: WidgetDateMode) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let delta_secs = (timestamp_ms - now_ms) / 1000;
    let abs_secs = delta_secs.unsigned_abs();
    match mode {
        WidgetDateMode::Relative | WidgetDateMode::Offset => {
            if abs_secs < 60 {
                if delta_secs >= 0 {
                    "just now".to_owned()
                } else {
                    format!("{abs_secs}s ago")
                }
            } else if abs_secs < 3600 {
                let mins = abs_secs / 60;
                if delta_secs >= 0 {
                    format!("in {mins}m")
                } else {
                    format!("{mins}m ago")
                }
            } else {
                let hours = abs_secs / 3600;
                if delta_secs >= 0 {
                    format!("in {hours}h")
                } else {
                    format!("{hours}h ago")
                }
            }
        }
        WidgetDateMode::Timer => {
            let mins = abs_secs / 60;
            let secs = abs_secs % 60;
            format!("{mins:02}:{secs:02}")
        }
        WidgetDateMode::Time => {
            let secs_in_day = timestamp_ms.div_euclid(1000).rem_euclid(86_400);
            let hours = secs_in_day / 3600;
            let mins = (secs_in_day % 3600) / 60;
            format!("{hours:02}:{mins:02}")
        }
        WidgetDateMode::Date => {
            let days = timestamp_ms.div_euclid(86_400_000);
            format!("Day {days}")
        }
    }
}
