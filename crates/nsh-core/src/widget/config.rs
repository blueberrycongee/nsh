//! Platform-neutral widget configuration (`nsh.json` `"widgets"` and
//! `"macos": { "widgets": { ... } }`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    element::PropEnum,
    geometry::{Points, Size, size_pt},
};

/// Minimum recommended refresh interval in seconds (5 minutes).
///
/// macOS WidgetKit enforces a system refresh budget (typically 15–60 minutes),
/// so intervals below 5 minutes are clamped and reported as warnings.
pub const MIN_REFRESH_INTERVAL_SECS: u64 = 300;

/// Supported widget size families across desktop and mobile hosts.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    PropEnum,
)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum WidgetFamily {
    /// Compact square card (170 × 170 pt on macOS).
    #[default]
    Small,
    /// Wide horizontal card (364 × 170 pt on macOS).
    Medium,
    /// Tall square-ish card (364 × 382 pt on macOS).
    Large,
}

impl WidgetFamily {
    /// Canonical macOS WidgetKit logical size in points.
    pub const fn macos_size(self) -> Size<Points> {
        match self {
            Self::Small => size_pt(170.0, 170.0),
            Self::Medium => size_pt(364.0, 170.0),
            Self::Large => size_pt(364.0, 382.0),
        }
    }

    /// Its canonical JSON / `nsh.json` name (`"small"`, `"medium"`, `"large"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    /// Its SwiftUI `WidgetFamily` case name (`".systemSmall"`, etc.).
    pub const fn swift_case(self) -> &'static str {
        match self {
            Self::Small => ".systemSmall",
            Self::Medium => ".systemMedium",
            Self::Large => ".systemLarge",
        }
    }

    /// Parses a family name (`"small"`, `"medium"`, `"large"`).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "large" => Some(Self::Large),
            _ => None,
        }
    }
}

/// One widget entry declared in `nsh.json` `"widgets"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WidgetEntryConfig {
    /// Stable widget identifier (`[a-z0-9][a-z0-9._-]{0,63}`), unique in the app.
    pub id: String,
    /// TSX entry file relative to `nsh.json` (e.g. `"widgets/PortfolioWidget.tsx"`).
    pub entry: String,
    /// Display name shown in the OS widget gallery.
    pub name: String,
    /// Short description shown in the OS widget gallery.
    #[serde(default)]
    pub description: String,
    /// Supported widget size families (at least one).
    pub families: Vec<WidgetFamily>,
    /// Optional periodic refresh hint (`"5m"`, `"15m"`, `"1h"`, `"6h"`, `"24h"`).
    #[serde(default)]
    pub refresh_interval: Option<String>,
    /// Whether the widget extension requires outbound network access.
    #[serde(default)]
    pub network: bool,
}

impl WidgetEntryConfig {
    /// Parsed refresh interval in seconds, clamped to [`MIN_REFRESH_INTERVAL_SECS`].
    pub fn refresh_interval_secs(&self) -> Result<Option<(u64, bool)>, String> {
        let Some(spec) = self.refresh_interval.as_deref() else {
            return Ok(None);
        };
        let secs = parse_refresh_interval(spec)?;
        if secs < MIN_REFRESH_INTERVAL_SECS {
            Ok(Some((MIN_REFRESH_INTERVAL_SECS, true)))
        } else {
            Ok(Some((secs, false)))
        }
    }
}

/// macOS-specific widget packaging overrides in `nsh.json` `"macos": { "widgets": { ... } }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MacosWidgetsConfig {
    /// Optional explicit App Group identifier. When omitted, `nsh package`
    /// derives `<TEAM_ID>.<app_id>.widgets` for signed builds or
    /// `nsh.local.<app_id>.widgets` for ad-hoc local builds.
    #[serde(default)]
    pub app_group: Option<String>,
}

/// Checks whether `id` is a valid widget identifier: ASCII
/// `[a-z0-9][a-z0-9._-]{0,63}`.
pub fn is_valid_widget_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// Parses a duration string like `"5m"`, `"15m"`, `"1h"`, `"6h"`, `"24h"`, `"30s"`.
/// Returns the duration in seconds.
pub fn parse_refresh_interval(spec: &str) -> Result<u64, String> {
    let spec = spec.trim();
    if spec.len() < 2 {
        return Err(format!(
            "invalid refreshInterval {spec:?}; expected a duration like \"15m\" or \"1h\""
        ));
    }
    let (digits, unit) = spec.split_at(spec.len() - 1);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "invalid refreshInterval {spec:?}; expected a positive integer followed by s, m, h, or d"
        ));
    }
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("invalid refreshInterval {spec:?}: number is too large"))?;
    if value == 0 {
        return Err(format!(
            "invalid refreshInterval {spec:?}: duration must be greater than zero"
        ));
    }
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => {
            return Err(format!(
                "invalid refreshInterval {spec:?}: unknown unit {unit:?} (use s, m, h, or d)"
            ));
        }
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("invalid refreshInterval {spec:?}: duration overflows"))
}

/// Validates a list of widget configurations from `nsh.json`.
///
/// Returns non-fatal warnings (such as refresh intervals below 5 minutes) on
/// success, or a descriptive error if any widget definition is invalid.
pub fn validate_widgets(entries: &[WidgetEntryConfig]) -> Result<Vec<String>, String> {
    let mut seen_ids = BTreeSet::new();
    let mut warnings = Vec::new();
    for (index, widget) in entries.iter().enumerate() {
        let prefix = format!("nsh.json widgets[{index}]");
        if !is_valid_widget_id(&widget.id) {
            return Err(format!(
                "{prefix}.id: {:?} must match [a-z0-9][a-z0-9._-]{{0,63}}",
                widget.id
            ));
        }
        if !seen_ids.insert(widget.id.as_str()) {
            return Err(format!("{prefix}.id: duplicate widget id {:?}", widget.id));
        }
        if widget.entry.trim().is_empty() {
            return Err(format!("{prefix}.entry: must not be empty"));
        }
        let entry_path = widget.entry.trim();
        if entry_path.starts_with('/')
            || entry_path.contains('\\')
            || entry_path.split('/').any(|seg| seg == "..")
        {
            return Err(format!(
                "{prefix}.entry: {:?} must be a relative path inside the app directory",
                widget.entry
            ));
        }
        if widget.name.trim().is_empty() {
            return Err(format!("{prefix}.name: must not be empty"));
        }
        if widget.families.is_empty() {
            return Err(format!(
                "{prefix}.families: must declare at least one family (\"small\", \"medium\", \"large\")"
            ));
        }
        let mut seen_families = BTreeSet::new();
        for family in &widget.families {
            if !seen_families.insert(*family) {
                return Err(format!(
                    "{prefix}.families: duplicate family {:?}",
                    family.as_str()
                ));
            }
        }
        if let Some((_, clamped)) = widget
            .refresh_interval_secs()
            .map_err(|e| format!("{prefix}.refreshInterval: {e}"))?
            && clamped
        {
            warnings.push(format!(
                "{prefix}.refreshInterval: {:?} is below the 5m minimum and will be clamped to 5m (WidgetKit enforces a system refresh budget)",
                widget.refresh_interval.as_deref().unwrap_or_default()
            ));
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_widget_configs_and_warns_on_short_refresh() {
        let widgets: Vec<WidgetEntryConfig> = serde_json::from_value(serde_json::json!([
            {
                "id": "portfolio",
                "entry": "widgets/PortfolioWidget.tsx",
                "name": "Portfolio",
                "description": "Watchlist prices.",
                "families": ["small", "medium", "large"],
                "refreshInterval": "15m",
                "network": false
            }
        ]))
        .unwrap();
        let warnings = validate_widgets(&widgets).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            widgets[0].refresh_interval_secs().unwrap(),
            Some((900, false))
        );

        // Short refresh interval warns and clamps to 5m.
        let short: Vec<WidgetEntryConfig> = serde_json::from_value(serde_json::json!([
            {
                "id": "ticker",
                "entry": "widgets/Ticker.tsx",
                "name": "Ticker",
                "families": ["small"],
                "refreshInterval": "30s"
            }
        ]))
        .unwrap();
        let warnings = validate_widgets(&short).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(short[0].refresh_interval_secs().unwrap(), Some((300, true)));
    }

    #[test]
    fn rejects_invalid_ids_duplicates_paths_and_families() {
        let bad_id: Vec<WidgetEntryConfig> = serde_json::from_value(serde_json::json!([
            { "id": "Bad_ID", "entry": "w.tsx", "name": "W", "families": ["small"] }
        ]))
        .unwrap();
        assert!(validate_widgets(&bad_id).unwrap_err().contains(".id:"));

        let dup: Vec<WidgetEntryConfig> = serde_json::from_value(serde_json::json!([
            { "id": "a", "entry": "a.tsx", "name": "A", "families": ["small"] },
            { "id": "a", "entry": "b.tsx", "name": "B", "families": ["medium"] }
        ]))
        .unwrap();
        assert!(
            validate_widgets(&dup)
                .unwrap_err()
                .contains("duplicate widget id")
        );

        let escape: Vec<WidgetEntryConfig> = serde_json::from_value(serde_json::json!([
            { "id": "a", "entry": "../outside.tsx", "name": "A", "families": ["small"] }
        ]))
        .unwrap();
        assert!(
            validate_widgets(&escape)
                .unwrap_err()
                .contains("relative path")
        );

        let empty_families: Vec<WidgetEntryConfig> = serde_json::from_value(serde_json::json!([
            { "id": "a", "entry": "a.tsx", "name": "A", "families": [] }
        ]))
        .unwrap();
        assert!(
            validate_widgets(&empty_families)
                .unwrap_err()
                .contains("at least one family")
        );
    }
}
