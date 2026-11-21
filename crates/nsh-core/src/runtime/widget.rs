//! `nsh/widget`'s `Widget` runtime module: pushing data and timelines to native
//! desktop/mobile widgets, reloading timelines, reading persisted widget data,
//! and receiving interactive `<WidgetAction>` events.

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use crate::{
    JsonValue,
    element::{
        CallCx, Callback, JsError, JsHost, JsIn, JsObject, JsOwned, JsValue, Local, PropEnum,
        PropError, Stop, System, TsTypes, codes, module,
    },
    widget::{WidgetActionEvent, WidgetReloadPolicy, WidgetTimelineDataEntry},
};

/// Platform-neutral native widget bridge (`import { Widget } from "nsh/widget"`).
#[module]
pub(crate) struct Widget;

#[module]
impl Widget {
    /// Returns `true` on platforms with a supported native widget host (macOS
    /// 14+ standalone), and `false` elsewhere.
    pub(crate) async fn available(sys: System) -> bool {
        sys.widget.as_ref().is_some_and(|host| host.available())
    }

    /// Pushes latest `data` for `widgetId`, evaluates its widget TSX into
    /// `WidgetTreeIr` for all configured families, persists it atomically to
    /// the shared container, and asks the OS to reload its timeline.
    pub(crate) async fn update(
        widget_id: String,
        data: JsonValue,
        options: Option<WidgetUpdateOptions>,
        sys: System,
    ) -> Result<(), JsError> {
        if widget_id.trim().is_empty() {
            return Err(JsError::Range(
                "Widget.update: `widgetId` must not be empty".into(),
            ));
        }
        let policy = parse_reload_policy(options.and_then(|o| o.reload_policy))?;
        let Some(host) = sys.widget else {
            return Err(unavailable("Widget.update"));
        };
        if !host.available() {
            return Err(unavailable("Widget.update"));
        }
        host.update(&widget_id, data, policy)
            .map_err(|msg| JsError::Error(msg).with_code(codes::HOST_ERROR))
    }

    /// Pushes a multi-entry scheduled timeline for `widgetId`. Entries remain
    /// active and advance automatically even after the main app quits.
    pub(crate) async fn set_timeline(
        widget_id: String,
        entries: Vec<WidgetTimelineEntryInput>,
        options: Option<WidgetUpdateOptions>,
        sys: System,
    ) -> Result<(), JsError> {
        if widget_id.trim().is_empty() {
            return Err(JsError::Range(
                "Widget.setTimeline: `widgetId` must not be empty".into(),
            ));
        }
        if entries.is_empty() {
            return Err(JsError::Range(
                "Widget.setTimeline: `entries` must contain at least one entry".into(),
            ));
        }
        let policy = parse_reload_policy(options.and_then(|o| o.reload_policy))?;
        let mut parsed = Vec::with_capacity(entries.len());
        for entry in entries {
            if !entry.date.is_finite() {
                return Err(JsError::Range(
                    "Widget.setTimeline: entry `date` must be a finite Unix timestamp in milliseconds".into(),
                ));
            }
            if let Some(rel) = entry.relevance
                && (!rel.is_finite() || !(0.0..=1.0).contains(&rel))
            {
                return Err(JsError::Range(
                    "Widget.setTimeline: entry `relevance` must be between 0 and 1".into(),
                ));
            }
            parsed.push(WidgetTimelineDataEntry {
                date_ms: entry.date as i64,
                data: entry.data,
                relevance: entry.relevance.map(|r| r as f32),
            });
        }
        let Some(host) = sys.widget else {
            return Err(unavailable("Widget.setTimeline"));
        };
        if !host.available() {
            return Err(unavailable("Widget.setTimeline"));
        }
        host.set_timeline(&widget_id, parsed, policy)
            .map_err(|msg| JsError::Error(msg).with_code(codes::HOST_ERROR))
    }

    /// Asks the OS to reload one widget's timeline, or all widgets if `widgetId`
    /// is omitted.
    pub(crate) async fn reload(widget_id: Option<String>, sys: System) -> Result<(), JsError> {
        let Some(host) = sys.widget else {
            return Ok(());
        };
        if !host.available() {
            return Ok(());
        }
        host.reload(widget_id.as_deref())
            .map_err(|msg| JsError::Error(msg).with_code(codes::HOST_ERROR))
    }

    /// Reads the last persisted data for `widgetId` (`null` if never written or
    /// unavailable).
    pub(crate) async fn get_data(
        widget_id: String,
        sys: System,
    ) -> Result<Option<JsonValue>, JsError> {
        if widget_id.trim().is_empty() {
            return Err(JsError::Range(
                "Widget.getData: `widgetId` must not be empty".into(),
            ));
        }
        let Some(host) = sys.widget else {
            return Ok(None);
        };
        if !host.available() {
            return Ok(None);
        }
        host.get_data(&widget_id)
            .map_err(|msg| JsError::Error(msg).with_code(codes::HOST_ERROR))
    }

    /// Listens to interactive actions triggered from desktop widgets.
    ///
    /// Actions performed while the main app was closed are queued in the shared
    /// container and replayed in order when the first listener attaches.
    /// Returns a function that removes the listener.
    pub(crate) fn on(
        event: WidgetEventKind,
        #[args(event)] listener: Callback<WidgetActionEventJs>,
        sys: System,
        cx: CallCx<'_>,
    ) -> Result<Stop, JsError> {
        let WidgetEventKind::Action = event;
        let state = sys.locals.get::<ActionListeners>();
        let id = state.next.get();
        state.next.set(id.wrapping_add(1));

        if let Some(host) = &sys.widget
            && host.available()
        {
            for pending in host.drain_pending_actions() {
                listener.post(WidgetActionEventJs::from(pending));
            }
        }
        for pending in state.unheard.take() {
            listener.post(WidgetActionEventJs::from(pending));
        }
        state.listeners.borrow_mut().push((id, listener));
        if !state.started.replace(true) {
            cx.start(deliver(Rc::downgrade(&state)));
        }
        let weak = Rc::downgrade(&state);
        Ok(Stop::new(move || {
            if let Some(state) = weak.upgrade() {
                state.listeners.borrow_mut().retain(|(i, _)| *i != id);
            }
        }))
    }
}

fn unavailable(call: &str) -> JsError {
    JsError::Error(format!(
        "{call}: native widgets are not available on this platform"
    ))
    .with_code(codes::UNAVAILABLE)
}

fn parse_reload_policy(
    value: Option<WidgetReloadPolicyValue>,
) -> Result<WidgetReloadPolicy, JsError> {
    let Some(value) = value else {
        return Ok(WidgetReloadPolicy::AtEnd);
    };
    match value.0 {
        JsonValue::String(ref s) if s == "atEnd" => Ok(WidgetReloadPolicy::AtEnd),
        JsonValue::String(ref s) if s == "never" => Ok(WidgetReloadPolicy::Never),
        JsonValue::Object(ref map) => {
            let Some(after) = map.get("after").and_then(JsonValue::as_f64) else {
                return Err(JsError::Range(
                    "reloadPolicy object must have a finite numeric `after` (milliseconds)".into(),
                ));
            };
            if !after.is_finite() || after <= 0.0 {
                return Err(JsError::Range(
                    "reloadPolicy.after must be a positive number of milliseconds".into(),
                ));
            }
            Ok(WidgetReloadPolicy::After(after as i64))
        }
        _ => Err(JsError::Type(
            "reloadPolicy must be \"atEnd\", \"never\", or { after: number }".into(),
        )),
    }
}

/// Options for `Widget.update` and `Widget.setTimeline`.
#[derive(Debug, Clone, Default, PartialEq, JsObject)]
#[prop(exact)]
pub(crate) struct WidgetUpdateOptions {
    /// Policy for when the OS should next ask for a timeline refresh.
    reload_policy: Option<WidgetReloadPolicyValue>,
}

/// JS reload policy union: `"atEnd" | "never" | { after: number }`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WidgetReloadPolicyValue(JsonValue);

impl JsValue for WidgetReloadPolicyValue {
    fn ts_type(_: &mut TsTypes) -> String {
        "\"atEnd\" | \"never\" | { after: number }".into()
    }

    fn from_js(value: JsIn<'_>) -> Result<Self, PropError> {
        Ok(Self(JsonValue::from_js(value)?))
    }

    fn to_js<'a>(&self, host: &'a dyn JsHost) -> Result<JsOwned<'a>, PropError> {
        self.0.to_js(host)
    }
}

/// One scheduled timeline entry passed to `Widget.setTimeline`.
#[derive(Debug, Clone, Default, PartialEq, JsObject)]
#[prop(exact)]
pub(crate) struct WidgetTimelineEntryInput {
    /// Unix timestamp in milliseconds when this entry becomes active.
    date: f64,
    /// JSON-serializable widget state passed to the widget component.
    data: JsonValue,
    /// Optional relevance score (`0..1`) for Smart Stack ranking.
    relevance: Option<f64>,
}

/// Events accepted by `Widget.on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PropEnum)]
pub(crate) enum WidgetEventKind {
    /// An interactive `<WidgetAction>` button was clicked on a desktop widget.
    #[default]
    Action,
}

/// Payload delivered to `Widget.on("action", listener)`.
#[derive(Debug, Clone, Default, PartialEq, JsObject)]
pub(crate) struct WidgetActionEventJs {
    /// Widget id from `nsh.json`.
    widget_id: String,
    /// Action name triggered by `<WidgetAction>`.
    action: String,
    /// Optional JSON-serializable payload attached to the button.
    #[prop(skip_none)]
    payload: Option<JsonValue>,
    /// Timestamp (ms) when the user clicked the action.
    timestamp: f64,
}

impl From<WidgetActionEvent> for WidgetActionEventJs {
    fn from(event: WidgetActionEvent) -> Self {
        Self {
            widget_id: event.widget_id,
            action: event.action,
            payload: event.payload,
            timestamp: event.timestamp as f64,
        }
    }
}

/// Per-VM listener list for `Widget.on("action", ...)`.
#[derive(Default)]
struct ActionListeners {
    listeners: RefCell<Vec<(u32, Callback<WidgetActionEventJs>)>>,
    next: Cell<u32>,
    started: Cell<bool>,
    unheard: RefCell<Vec<WidgetActionEvent>>,
}

impl Local for ActionListeners {
    fn close(&self) {
        self.listeners.borrow_mut().clear();
    }
}

async fn deliver(state: Weak<ActionListeners>) {
    loop {
        let event = crate::element::next_widget_action().await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let listeners = state.listeners.borrow();
        if listeners.is_empty() {
            state.unheard.borrow_mut().push(event);
            continue;
        }
        let payload = WidgetActionEventJs::from(event);
        for (_, listener) in &*listeners {
            listener.post(payload.clone());
        }
    }
}
