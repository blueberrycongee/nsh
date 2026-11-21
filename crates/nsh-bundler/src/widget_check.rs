//! Build-time AST validation for widget TSX entries (`nsh.json` `"widgets"`).
//!
//! Widget extensions (macOS WidgetKit, Windows Adaptive Cards, Android Glance)
//! render a restricted declarative subset of NSH primitives. Checking widget
//! sources statically during `nsh check`, `nsh build`, `nsh dev`, and
//! `nsh package` reports unsupported elements, interactive props, and runtime
//! modules with exact `line:column` positions and actionable alternatives.

use std::fmt;

use oxc::{
    allocator::Allocator,
    ast::ast::{
        ImportDeclaration, ImportDeclarationSpecifier, JSXAttribute, JSXAttributeName,
        JSXElementName, JSXOpeningElement,
    },
    ast_visit::{Visit, walk},
    parser::Parser,
    span::SourceType,
};

/// A static validation error found in a widget module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WidgetDiagnostic {
    /// 1-based line number in the source file.
    pub line: usize,
    /// 1-based column number in the source file.
    pub column: usize,
    /// Stable diagnostic code (`WIDGET_FORBIDDEN_ELEMENT`, etc.).
    pub code: &'static str,
    /// Human-readable error message.
    pub message: String,
    /// Actionable suggestion for fixing the error.
    pub suggestion: String,
}

impl fmt::Display for WidgetDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} [{}]: {} ({})",
            self.line, self.column, self.code, self.message, self.suggestion
        )
    }
}

/// JSX element names forbidden inside widget trees.
const FORBIDDEN_ELEMENTS: &[(&str, &str)] = &[
    ("Input", "widgets do not support free-form text input"),
    (
        "Scroll",
        "widgets are fixed-size surfaces and cannot scroll",
    ),
    (
        "List",
        "use a fixed small <Each> or array map instead of virtualized <List>",
    ),
    ("WebView", "widgets cannot embed web views"),
    (
        "Video",
        "widgets cannot play video; use a static <Image> thumbnail",
    ),
    (
        "Canvas",
        "use declarative <View>, <Text>, <Image>, and <Progress> primitives",
    ),
    ("Shader", "widgets cannot run custom GPU shaders"),
    (
        "ShaderEffect",
        "widgets cannot run custom GPU shader effects",
    ),
    (
        "ShaderTransition",
        "widgets cannot run custom GPU shader transitions",
    ),
    (
        "ScreenEffect",
        "widgets cannot run custom GPU screen effects",
    ),
    (
        "ScreenTransition",
        "widgets cannot run custom GPU screen transitions",
    ),
    (
        "NavStack",
        "use <Link href=\"...\"> to deep-link into main app screens",
    ),
    (
        "NavPage",
        "use <Link href=\"...\"> to deep-link into main app screens",
    ),
    ("Dialog", "widgets cannot present modal dialogs"),
    ("AlertDialog", "widgets cannot present alert dialogs"),
    ("Popover", "widgets cannot present popovers"),
    ("Tooltip", "widgets cannot present tooltips"),
    (
        "Menu",
        "use <WidgetAction> buttons for discrete desktop actions",
    ),
    ("ContextMenu", "widgets use the system widget context menu"),
    ("Select", "use <WidgetAction> buttons to cycle options"),
    ("Combobox", "widgets do not support combobox input"),
    ("Slider", "widgets do not support continuous drag sliders"),
    ("Editor", "widgets do not support code editors"),
    ("DiffView", "widgets do not support diff views"),
    (
        "DataTable",
        "use compact <View> rows and columns inside widgets",
    ),
    ("RichEditor", "widgets do not support rich text editors"),
    ("Terminal", "widgets do not support terminal emulators"),
    ("DockArea", "widgets cannot host dockable workspaces"),
    (
        "Window",
        "widgets render inside the OS widget surface, not standalone windows",
    ),
];

/// Props/events forbidden on elements inside widget trees.
const FORBIDDEN_PROPS: &[(&str, &str)] = &[
    (
        "onPress",
        "use <WidgetAction action=\"...\"> for desktop light interaction or <Link href=\"...\"> for deep links",
    ),
    (
        "onLongPress",
        "widgets only support discrete tap links and <WidgetAction>",
    ),
    (
        "onPressDown",
        "widgets do not deliver raw press-down events",
    ),
    ("onPressUp", "widgets do not deliver raw press-up events"),
    ("onFocus", "widgets do not manage keyboard focus"),
    ("onBlur", "widgets do not manage keyboard focus"),
    ("onScroll", "widgets do not scroll"),
    ("onWheel", "widgets do not deliver wheel events"),
    ("onPinch", "widgets do not deliver pinch gestures"),
    (
        "selectable",
        "text selection is not supported inside system widgets",
    ),
    (
        "focusable",
        "keyboard focus management is not supported inside system widgets",
    ),
    (
        "animation",
        "continuous animations are not supported inside system widgets",
    ),
    (
        "transition",
        "CSS/layout transitions are not supported inside system widgets",
    ),
];

/// Module specifiers forbidden inside widget code.
const FORBIDDEN_MODULES: &[(&str, &str)] = &[
    (
        "nsh/desktop",
        "desktop window/menu/tray APIs are unavailable inside widgets",
    ),
    (
        "nsh/mobile",
        "mobile native controls are unavailable inside widgets",
    ),
    (
        "nsh/updater",
        "app updater APIs are unavailable inside widgets",
    ),
    (
        "fs/promises",
        "direct filesystem APIs are unavailable inside widgets; pass data via Widget.update",
    ),
    ("process", "process APIs are unavailable inside widgets"),
    (
        "websocket",
        "persistent WebSocket connections are unavailable inside widgets",
    ),
];

/// Named imports from `"nsh"` forbidden inside widget code.
const FORBIDDEN_NSH_IMPORTS: &[(&str, &str)] = &[
    (
        "AudioPlayer",
        "audio playback is unavailable inside widgets",
    ),
    ("Sound", "sound playback is unavailable inside widgets"),
    ("Speech", "speech recognition is unavailable inside widgets"),
    (
        "Picker",
        "file/image pickers are unavailable inside widgets",
    ),
    ("Clipboard", "clipboard APIs are unavailable inside widgets"),
    (
        "PanResponder",
        "continuous touch responders are unavailable inside widgets",
    ),
    (
        "Animated",
        "continuous native animations are unavailable inside widgets",
    ),
];

/// Checks a widget TSX/TS/JSX/JS source file on disk against the widget
/// allow-list.
pub fn check_widget_file(
    path: &std::path::Path,
    allow_network: bool,
) -> Result<Vec<WidgetDiagnostic>, std::io::Error> {
    let source = std::fs::read_to_string(path)?;
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::tsx());
    Ok(check_widget_source(&source, source_type, allow_network))
}

/// Checks a widget TSX/TS/JSX/JS source module against the widget allow-list.
///
/// When `allow_network` is `false` (default when `network: false` in `nsh.json`),
/// importing `"net"` is also reported as an error.
pub fn check_widget_source(
    source: &str,
    source_type: SourceType,
    allow_network: bool,
) -> Vec<WidgetDiagnostic> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let mut visitor = WidgetChecker {
        source,
        allow_network,
        diagnostics: Vec::new(),
    };
    visitor.visit_program(&parsed.program);
    visitor.diagnostics
}

struct WidgetChecker<'s> {
    source: &'s str,
    allow_network: bool,
    diagnostics: Vec<WidgetDiagnostic>,
}

impl WidgetChecker<'_> {
    fn push(&mut self, offset: u32, code: &'static str, message: String, suggestion: &str) {
        let (line, column) = crate::compile::position(self.source, offset);
        self.diagnostics.push(WidgetDiagnostic {
            line,
            column,
            code,
            message,
            suggestion: suggestion.to_owned(),
        });
    }
}

impl<'a> Visit<'a> for WidgetChecker<'_> {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        let spec = decl.source.value.as_str();
        if let Some((_, suggestion)) = FORBIDDEN_MODULES.iter().find(|(m, _)| *m == spec) {
            self.push(
                decl.source.span.start,
                "WIDGET_FORBIDDEN_IMPORT",
                format!("import `{spec}` is not allowed in widget code"),
                suggestion,
            );
        } else if spec == "net" && !self.allow_network {
            self.push(
                decl.source.span.start,
                "WIDGET_NETWORK_DISABLED",
                "import `net` requires `\"network\": true` on this widget in nsh.json".to_owned(),
                "set `\"network\": true` in nsh.json or fetch data in the main app and push via Widget.update",
            );
        } else if spec == "nsh"
            && let Some(specifiers) = &decl.specifiers
        {
            for item in specifiers {
                if let ImportDeclarationSpecifier::ImportSpecifier(named) = item {
                    let imported = named.imported.name().as_str();
                    if let Some((_, suggestion)) =
                        FORBIDDEN_NSH_IMPORTS.iter().find(|(n, _)| *n == imported)
                    {
                        self.push(
                            named.span.start,
                            "WIDGET_FORBIDDEN_IMPORT",
                            format!("`{imported}` from \"nsh\" is not allowed in widget code"),
                            suggestion,
                        );
                    }
                }
            }
        }
        walk::walk_import_declaration(self, decl);
    }

    fn visit_jsx_opening_element(&mut self, elem: &JSXOpeningElement<'a>) {
        let (name, start) = match &elem.name {
            JSXElementName::Identifier(ident) => (ident.name.as_str(), ident.span.start),
            JSXElementName::IdentifierReference(ident) => (ident.name.as_str(), ident.span.start),
            _ => {
                walk::walk_jsx_opening_element(self, elem);
                return;
            }
        };
        if let Some((_, suggestion)) = FORBIDDEN_ELEMENTS.iter().find(|(e, _)| *e == name) {
            self.push(
                start,
                "WIDGET_FORBIDDEN_ELEMENT",
                format!("<{name}> is not supported inside widgets"),
                suggestion,
            );
        }
        walk::walk_jsx_opening_element(self, elem);
    }

    fn visit_jsx_attribute(&mut self, attr: &JSXAttribute<'a>) {
        if let JSXAttributeName::Identifier(ident) = &attr.name {
            let name = ident.name.as_str();
            if let Some((_, suggestion)) = FORBIDDEN_PROPS.iter().find(|(p, _)| *p == name) {
                self.push(
                    ident.span.start,
                    "WIDGET_FORBIDDEN_PROP",
                    format!("prop `{name}` is not supported inside widgets"),
                    suggestion,
                );
            } else if name.starts_with("onPan")
                || name.starts_with("onMouse")
                || name.starts_with("onKey")
            {
                self.push(
                    ident.span.start,
                    "WIDGET_FORBIDDEN_PROP",
                    format!("event prop `{name}` is not supported inside widgets"),
                    "use <WidgetAction> for discrete actions or <Link> for deep links",
                );
            }
        }
        walk::walk_jsx_attribute(self, attr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_valid_widget_tsx() {
        let source = r#"
import { Image, Text, View } from "nsh";
import { Link, Progress } from "nsh/base";
import { defineWidget, WidgetAction, WidgetDate } from "nsh/widget";

export default defineWidget<{ symbol: string }>({
  render({ data }) {
    return (
      <View class="flex-1 p-4 bg-card">
        <Text class="text-sm font-semibold">{data?.symbol ?? "AAPL"}</Text>
        <Progress value={72} />
        <Link href="pulse://symbol/AAPL"><Text>Open</Text></Link>
        <WidgetAction action="refresh"><Text>Refresh</Text></WidgetAction>
        <WidgetDate date={1795280000000} mode="relative" />
      </View>
    );
  },
});
"#;
        let diags = check_widget_source(source, SourceType::tsx(), false);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn reports_forbidden_elements_props_and_imports_with_positions() {
        let source = r#"import { View, Animated } from "nsh";
import { Window } from "nsh/desktop";
import { Input, Scroll } from "nsh/base";
import * as net from "net";

export function Bad() {
  return (
    <Scroll>
      <View onPress={() => {}} onMouseEnter={() => {}} transition="all">
        <Input />
      </View>
    </Scroll>
  );
}
"#;
        let diags = check_widget_source(source, SourceType::tsx(), false);
        let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert!(codes.contains(&"WIDGET_FORBIDDEN_IMPORT"), "{diags:?}");
        assert!(codes.contains(&"WIDGET_NETWORK_DISABLED"), "{diags:?}");
        assert!(codes.contains(&"WIDGET_FORBIDDEN_ELEMENT"), "{diags:?}");
        assert!(codes.contains(&"WIDGET_FORBIDDEN_PROP"), "{diags:?}");
        // Verify exact 1-based line numbers.
        let scroll = diags
            .iter()
            .find(|d| d.message.contains("<Scroll>"))
            .unwrap();
        assert_eq!(scroll.line, 8);
        let on_press = diags
            .iter()
            .find(|d| d.message.contains("`onPress`"))
            .unwrap();
        assert_eq!(on_press.line, 9);
        assert!(on_press.suggestion.contains("<WidgetAction"));

        // When network is enabled in nsh.json, `import * as net from "net"` is allowed.
        let with_net =
            check_widget_source("import * as net from \"net\";", SourceType::tsx(), true);
        assert!(with_net.is_empty(), "{with_net:?}");
    }
}
