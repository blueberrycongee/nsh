//! Shadow-tree extraction from [`Core`] into platform-neutral
//! [`WidgetFamilyTree`] IR.
//!
//! Because `Core` already resolves Tailwind classes, design tokens, CSS
//! inheritance, and Taffy layout in both Light and Dark appearances without
//! changing `NodeId`s, extracting a widget snapshot takes two lightweight
//! commits on the same tree (`Appearance::Light` then `Appearance::Dark`) and
//! pairs every color by `NodeId` into a dual-appearance [`WidgetColor`].

use rustc_hash::FxHashMap;
use taffy::{
    AlignItems, Dimension, Display, FlexDirection, JustifyContent, LengthPercentage, Position,
};

use super::{Core, Key, Kind, TextAttachments};
use crate::{
    AccessibilityRole, AttributedText, Color, NodeId, ROOT_ID, ResizeMode, TextAlign, TextEngine,
    ViewStyle,
    geometry::{Points, Size},
    style::{FamilyName, TextDecoration},
    widget::{
        WidgetAccentedImageMode, WidgetAxis, WidgetBoxStyle, WidgetColor, WidgetCrossAlign,
        WidgetEdges, WidgetFamilyTree, WidgetFill, WidgetGradientStop, WidgetImageSource,
        WidgetLayout, WidgetMainAlign, WidgetNode, WidgetResizeMode, WidgetTextAlign,
        WidgetTextRun, WidgetTextStyle,
    },
};

/// Snapshot of resolved visual styles for every live node in one appearance
/// (Light or Dark), captured by [`Core::capture_widget_appearance`].
#[derive(Debug, Default)]
pub struct WidgetAppearanceCapture {
    nodes: FxHashMap<NodeId, CapturedNode>,
}

#[derive(Debug, Clone)]
struct CapturedNode {
    view_style: ViewStyle,
    text: Option<AttributedText>,
    text_color: Option<Color>,
    font_size: Points,
    font_weight: u16,
    italic: bool,
    monospaced: bool,
    underline: bool,
    strikethrough: bool,
    letter_spacing: Option<Points>,
    size: Size<Points>,
}

impl<M: TextEngine> Core<M> {
    /// Captures the resolved styles and attributed text runs of every live node
    /// under `ROOT_ID` in the core's current appearance (`Light` or `Dark`).
    ///
    /// Call once after `commit` in `Appearance::Light` and once after `commit`
    /// in `Appearance::Dark`, then pass both captures to
    /// [`Self::extract_widget_family_tree`].
    pub fn capture_widget_appearance(&mut self) -> WidgetAppearanceCapture {
        let mut capture = WidgetAppearanceCapture::default();
        if let Ok(root_key) = self.key(ROOT_ID) {
            self.capture_subtree(root_key, &mut capture);
        }
        capture
    }

    fn capture_subtree(&mut self, key: Key, out: &mut WidgetAppearanceCapture) {
        let (id, kind, children, view_style, frame_size) = {
            let node = &self.nodes[key as usize];
            (
                node.id,
                node.kind,
                node.children.clone(),
                (*node.view_style).clone(),
                node.frame.size,
            )
        };
        let inherited = self.inherited_text_style(key);
        let resolved_style = inherited.inherit(&self.nodes[key as usize].text_style);
        let monospaced = resolved_style.font_family.is_some_and(|family| {
            let name = family.name();
            crate::font_families(&name).any(|f| matches!(f, FamilyName::Monospace))
        });
        let text = if kind == Kind::Text {
            Some(self.collect_widget_text(key, resolved_style))
        } else {
            None
        };
        let deco = resolved_style.decoration;
        out.nodes.insert(
            id,
            CapturedNode {
                view_style,
                text,
                text_color: Some(resolved_style.color),
                font_size: resolved_style.font_size,
                font_weight: resolved_style.font_weight,
                italic: resolved_style.italic,
                monospaced,
                underline: deco.underline,
                strikethrough: deco.line_through,
                letter_spacing: (resolved_style.letter_spacing.0 != 0.0)
                    .then_some(resolved_style.letter_spacing),
                size: frame_size,
            },
        );
        for child in children {
            if self.nodes.contains(child as usize) {
                self.capture_subtree(child, out);
            }
        }
    }

    fn collect_widget_text(
        &mut self,
        key: Key,
        base: crate::text::ResolvedTextStyle,
    ) -> AttributedText {
        let node = &self.nodes[key as usize];
        let max_lines = node
            .class_props
            .as_deref()
            .and_then(|c| c.number_of_lines)
            .unwrap_or(0);
        let (align, physical) = node
            .text_style
            .text_align
            .map(|a| (a, node.text_style.text_align_physical))
            .or_else(|| self.inherited_text_align(key))
            .unwrap_or_default();
        let mut text = AttributedText {
            align: super::directed_align(align, node.rtl && !physical),
            max_lines,
            ..AttributedText::default()
        };
        let baselines = Vec::new();
        let mut parts = TextAttachments {
            alignments: crate::AttachmentAlignments::default(),
            spacers: Vec::new(),
            baselines: &baselines,
            next_baseline: 0,
        };
        self.collect_text(key, base, &mut text, &mut parts, false);
        text
    }

    /// Walks the committed shadow tree side-by-side with `light` and `dark`
    /// captures to build a dual-appearance [`WidgetFamilyTree`].
    pub fn extract_widget_family_tree(
        &self,
        light: &WidgetAppearanceCapture,
        dark: &WidgetAppearanceCapture,
        url: Option<String>,
    ) -> WidgetFamilyTree {
        let Ok(root_key) = self.key(ROOT_ID) else {
            return WidgetFamilyTree {
                url,
                ..WidgetFamilyTree::default()
            };
        };
        // `mount()` on desktop wraps root content in an implicit `Kind::Modal`
        // window host under `ROOT_ID`; descend through any such wrapper to
        // reach the widget's own root element.
        let mut container_key = root_key;
        loop {
            let visible: Vec<Key> = self.nodes[container_key as usize]
                .children
                .iter()
                .copied()
                .filter(|&k| self.is_widget_visible(k))
                .collect();
            if visible.len() == 1 && self.nodes[visible[0] as usize].kind == Kind::Modal {
                container_key = visible[0];
            } else {
                break;
            }
        }
        let visible_children: Vec<Key> = self.nodes[container_key as usize]
            .children
            .iter()
            .copied()
            .filter(|&k| self.is_widget_visible(k))
            .collect();
        let start_key = if visible_children.len() == 1 {
            visible_children[0]
        } else {
            container_key
        };
        let start_id = self.nodes[start_key as usize].id;
        let root_hints = self
            .widget_nodes
            .get(&start_id)
            .and_then(|m| m.hints.clone());

        let mut root = self
            .extract_widget_node(start_key, light, dark)
            .unwrap_or_else(|| WidgetNode::Box {
                layout: WidgetLayout::default(),
                style: WidgetBoxStyle::default(),
                children: Vec::new(),
            });

        // Lift root container background into `.containerBackground(for: .widget)`
        // unless the developer explicitly opted out with `widget={{ containerBackground: false }}`.
        let lift_bg = root_hints
            .as_ref()
            .and_then(|h| h.container_background)
            .unwrap_or(true);
        let mut container_background = None;
        if lift_bg && let WidgetNode::Box { style, .. } = &mut root {
            container_background = style.background.take();
        }

        WidgetFamilyTree {
            url,
            container_background,
            root,
        }
    }

    fn is_widget_visible(&self, key: Key) -> bool {
        if !self.nodes.contains(key as usize) {
            return false;
        }
        let node = &self.nodes[key as usize];
        if node.kind == Kind::Overlay || node.is_overlay() || node.view_style.hidden {
            return false;
        }
        if let Some(layout_id) = node.layout
            && self.layout.style(layout_id).display == Display::None
        {
            return false;
        }
        true
    }

    fn extract_widget_node(
        &self,
        key: Key,
        light: &WidgetAppearanceCapture,
        dark: &WidgetAppearanceCapture,
    ) -> Option<WidgetNode> {
        if !self.is_widget_visible(key) {
            return None;
        }
        let node = &self.nodes[key as usize];
        let id = node.id;
        let l = light.nodes.get(&id);
        let d = dark.nodes.get(&id);
        let meta = self.widget_nodes.get(&id);
        let hints = meta.and_then(|m| m.hints.as_ref());
        let layout = self.extract_layout(key);

        // 1. `<WidgetDate>` live OS-driven date node.
        if let Some(date_meta) = meta.and_then(|m| m.date.as_ref()) {
            let color =
                pair_option_color(l.and_then(|c| c.text_color), d.and_then(|c| c.text_color));
            let style = WidgetTextStyle {
                font_size: l.map_or(Points(14.0), |c| c.font_size),
                font_weight: l.map_or(400, |c| c.font_weight),
                italic: l.is_some_and(|c| c.italic),
                monospaced: l.is_some_and(|c| c.monospaced),
                color,
                underline: l.is_some_and(|c| c.underline),
                strikethrough: l.is_some_and(|c| c.strikethrough),
                letter_spacing: l.and_then(|c| c.letter_spacing),
            };
            let accentable = hints
                .and_then(|h| h.accentable)
                .unwrap_or_else(|| color.is_some_and(|c| c.is_chromatic()));
            return Some(WidgetNode::Date {
                timestamp_ms: date_meta.timestamp_ms,
                mode: date_meta.mode,
                style,
                accentable,
                layout,
            });
        }

        // 2. `<Progress>` (`nsh/base`) detected via `AccessibilityRole::Progressbar`.
        if let Some(props) = node.props.as_deref()
            && props.accessibility.role == AccessibilityRole::Progressbar
        {
            return Some(self.extract_progress(key, layout, light, dark));
        }

        // 3. `<Text>` block with dual-appearance styled runs.
        if node.kind == Kind::Text {
            let mut runs = Vec::new();
            let mut align = WidgetTextAlign::Leading;
            let mut line_limit = None;
            if let Some(lt) = l.and_then(|c| c.text.as_ref()) {
                let dt = d.and_then(|c| c.text.as_ref());
                align = map_text_align(lt.align);
                if lt.max_lines > 0 {
                    line_limit = Some(lt.max_lines);
                }
                if lt.spans.is_empty() && !lt.text.is_empty() {
                    let color = pair_option_color(
                        l.and_then(|c| c.text_color),
                        d.and_then(|c| c.text_color),
                    );
                    runs.push(WidgetTextRun {
                        text: lt.text.clone(),
                        style: WidgetTextStyle {
                            font_size: l.map_or(Points(14.0), |c| c.font_size),
                            font_weight: l.map_or(400, |c| c.font_weight),
                            italic: l.is_some_and(|c| c.italic),
                            monospaced: l.is_some_and(|c| c.monospaced),
                            color,
                            underline: l.is_some_and(|c| c.underline),
                            strikethrough: l.is_some_and(|c| c.strikethrough),
                            letter_spacing: l.and_then(|c| c.letter_spacing),
                        },
                    });
                } else {
                    for (idx, span) in lt.spans.iter().enumerate() {
                        if span.style.hidden {
                            continue;
                        }
                        let slice = lt
                            .text
                            .get(span.start as usize..span.end as usize)
                            .unwrap_or_default();
                        if slice.is_empty() {
                            continue;
                        }
                        let dark_color = dt
                            .and_then(|t| t.spans.get(idx))
                            .map(|s| s.style.color)
                            .unwrap_or(span.style.color);
                        let color = WidgetColor::from_packed(span.style.color, dark_color);
                        let monospaced = span.style.font_family.is_some_and(|family| {
                            let name = family.name();
                            crate::font_families(&name).any(|f| matches!(f, FamilyName::Monospace))
                        });
                        let deco: TextDecoration = span.style.decoration;
                        runs.push(WidgetTextRun {
                            text: slice.to_owned(),
                            style: WidgetTextStyle {
                                font_size: span.style.font_size,
                                font_weight: span.style.font_weight,
                                italic: span.style.italic,
                                monospaced,
                                color: Some(color),
                                underline: deco.underline,
                                strikethrough: deco.line_through,
                                letter_spacing: (span.style.letter_spacing.0 != 0.0)
                                    .then_some(span.style.letter_spacing),
                            },
                        });
                    }
                }
            }
            let accentable = hints.and_then(|h| h.accentable).unwrap_or_else(|| {
                runs.iter()
                    .any(|r| r.style.color.is_some_and(|c| c.is_chromatic()))
            });
            return Some(WidgetNode::Text {
                runs,
                align,
                line_limit,
                accentable,
                layout,
            });
        }

        // 4. `<Image>`.
        if node.kind == Kind::Image {
            let source = node
                .image
                .as_deref()
                .and_then(|img| img.source.as_ref())
                .map(|src| map_image_source(&src.uri))
                .unwrap_or_else(|| WidgetImageSource::Asset {
                    path: String::new(),
                });
            let resize_mode = map_resize_mode(node.object_fit.unwrap_or_default());
            let tint = pair_option_color(
                l.and_then(|c| c.view_style.tint_color),
                d.and_then(|c| c.view_style.tint_color),
            );
            let accented_mode = hints
                .and_then(|h| h.accented_rendering_mode)
                .unwrap_or(WidgetAccentedImageMode::Desaturated);
            let style = extract_box_style(l, d, hints);
            return Some(WidgetNode::Image {
                source,
                resize_mode,
                tint,
                accented_mode,
                style,
                layout,
            });
        }

        // 5. Container (`<View>`, `<Link>`, `<WidgetAction>`).
        let style = extract_box_style(l, d, hints);
        let mut children = Vec::new();
        for &child_key in &node.children {
            if let Some(child) = self.extract_widget_node(child_key, light, dark) {
                children.push(child);
            }
        }

        let box_node = WidgetNode::Box {
            layout,
            style,
            children,
        };

        // Wrap in `<WidgetAction>` if this node carries action metadata.
        if let Some(action_meta) = meta.and_then(|m| m.action.as_ref()) {
            return Some(WidgetNode::Action {
                action: action_meta.action.clone(),
                payload_json: action_meta.payload_json.clone(),
                patch_json: action_meta.patch_json.clone(),
                accessibility_label: action_meta.accessibility_label.clone(),
                child: Box::new(box_node),
            });
        }

        // Wrap in `<Link>` if this node is an `nsh/base` Link with an `href`.
        if let Some(props) = node.props.as_deref()
            && props.accessibility.role == AccessibilityRole::Link
            && let Some(href) = props.href.as_deref()
        {
            return Some(WidgetNode::Link {
                href: href.to_owned(),
                accessibility_label: props.accessibility.label.as_deref().map(str::to_owned),
                child: Box::new(box_node),
            });
        }

        Some(box_node)
    }

    fn extract_progress(
        &self,
        key: Key,
        mut layout: WidgetLayout,
        light: &WidgetAppearanceCapture,
        dark: &WidgetAppearanceCapture,
    ) -> WidgetNode {
        let node = &self.nodes[key as usize];
        let value = node
            .props
            .as_deref()
            .and_then(|p| p.accessibility.value.as_ref())
            .and_then(|v| {
                let now = v.now?;
                let max = v.max.unwrap_or(100.0);
                if max > 0.0 {
                    Some((now / max).clamp(0.0, 1.0))
                } else {
                    Some(0.0)
                }
            })
            .unwrap_or(0.0);

        let track_key = node.children.first().copied();
        let indicator_key = track_key
            .and_then(|tk| self.nodes.get(tk as usize))
            .and_then(|t| t.children.first().copied());

        // Track color can come from either the Progress root or its track child.
        let root_bg = extract_solid_bg(light.nodes.get(&node.id), dark.nodes.get(&node.id));
        let track_bg = track_key.and_then(|tk| {
            let tid = self.nodes[tk as usize].id;
            extract_solid_bg(light.nodes.get(&tid), dark.nodes.get(&tid))
        });
        let indicator_bg = indicator_key.and_then(|ik| {
            let iid = self.nodes[ik as usize].id;
            extract_solid_bg(light.nodes.get(&iid), dark.nodes.get(&iid))
        });
        let corner_radius = track_key
            .and_then(|tk| light.nodes.get(&self.nodes[tk as usize].id))
            .map(|c| c.view_style.border_radius)
            .filter(|r| r.0 > 0.0)
            .or_else(|| {
                light
                    .nodes
                    .get(&node.id)
                    .map(|c| c.view_style.border_radius)
                    .filter(|r| r.0 > 0.0)
            });

        if layout.height.is_none()
            && let Some(tk) = track_key
        {
            layout.height = self.extract_layout(tk).height;
        }

        WidgetNode::Progress {
            value,
            track_color: track_bg.or(root_bg),
            indicator_color: indicator_bg,
            corner_radius,
            layout,
        }
    }

    fn extract_layout(&self, key: Key) -> WidgetLayout {
        let node = &self.nodes[key as usize];
        let mut out = WidgetLayout::default();
        let Some(layout_id) = node.layout else {
            return out;
        };
        let style = self.layout.style(layout_id);

        // If any visible child is `position: absolute`, model the container as a ZStack.
        let has_absolute_child = node.children.iter().any(|&ck| {
            self.nodes
                .get(ck as usize)
                .and_then(|c| c.layout)
                .is_some_and(|lid| self.layout.style(lid).position == Position::Absolute)
        });
        out.axis = if has_absolute_child {
            WidgetAxis::Stack
        } else {
            match style.flex_direction {
                FlexDirection::Row | FlexDirection::RowReverse => WidgetAxis::Row,
                FlexDirection::Column | FlexDirection::ColumnReverse => WidgetAxis::Column,
            }
        };

        out.main_align = match style.justify_content {
            c if c == JustifyContent::CENTER => WidgetMainAlign::Center,
            c if c == JustifyContent::FLEX_END || c == JustifyContent::END => WidgetMainAlign::End,
            c if c == JustifyContent::SPACE_BETWEEN => WidgetMainAlign::SpaceBetween,
            c if c == JustifyContent::SPACE_AROUND => WidgetMainAlign::SpaceAround,
            c if c == JustifyContent::SPACE_EVENLY => WidgetMainAlign::SpaceEvenly,
            _ => WidgetMainAlign::Start,
        };
        out.cross_align = match style.align_items {
            c if c == AlignItems::CENTER => WidgetCrossAlign::Center,
            c if c == AlignItems::FLEX_END || c == AlignItems::END => WidgetCrossAlign::End,
            c if c == AlignItems::STRETCH => WidgetCrossAlign::Stretch,
            _ => WidgetCrossAlign::Start,
        };
        out.gap = match out.axis {
            WidgetAxis::Row => length_pct_points(style.gap.width),
            WidgetAxis::Column | WidgetAxis::Stack => length_pct_points(style.gap.height),
        };
        out.padding = WidgetEdges {
            top: length_pct_points(style.padding.top),
            right: length_pct_points(style.padding.right),
            bottom: length_pct_points(style.padding.bottom),
            left: length_pct_points(style.padding.left),
        };
        out.width = dimension_points(style.size.width);
        out.height = dimension_points(style.size.height);
        out.min_width = dimension_points(style.min_size.width);
        out.min_height = dimension_points(style.min_size.height);
        out.max_width = dimension_points(style.max_size.width);
        out.max_height = dimension_points(style.max_size.height);
        out.flex_grow = style.flex_grow;
        out
    }
}

fn length_pct_points(value: LengthPercentage) -> Points {
    let raw = value.into_raw();
    if raw.tag() == taffy::CompactLength::LENGTH_TAG {
        Points(raw.value())
    } else {
        Points(0.0)
    }
}

fn dimension_points(value: impl Into<Dimension>) -> Option<Points> {
    let raw = value.into().into_raw();
    if raw.tag() == taffy::CompactLength::LENGTH_TAG {
        Some(Points(raw.value()))
    } else {
        None
    }
}

fn pair_option_color(light: Option<Color>, dark: Option<Color>) -> Option<WidgetColor> {
    match (light, dark) {
        (Some(l), Some(d)) => Some(WidgetColor::from_packed(l, d)),
        (Some(l), None) => Some(WidgetColor::from_packed(l, l)),
        (None, Some(d)) => Some(WidgetColor::from_packed(d, d)),
        (None, None) => None,
    }
}

fn extract_solid_bg(
    light: Option<&CapturedNode>,
    dark: Option<&CapturedNode>,
) -> Option<WidgetColor> {
    let l = light
        .map(|c| c.view_style.background_color)
        .unwrap_or(Color::TRANSPARENT);
    let d = dark.map(|c| c.view_style.background_color).unwrap_or(l);
    if l.is_transparent() && d.is_transparent() {
        None
    } else {
        Some(WidgetColor::from_packed(l, d))
    }
}

fn extract_fill(light: Option<&CapturedNode>, dark: Option<&CapturedNode>) -> Option<WidgetFill> {
    // Prefer linear gradient if present on `effects.background_images`.
    if let Some(l_node) = light
        && let Some(effects) = l_node.view_style.effects.as_deref()
        && let Some(l_grad) = effects.background_images.first()
        && let Some(angle) = l_grad.kind.linear_angle(l_node.size)
    {
        let d_grad = dark
            .and_then(|c| c.view_style.effects.as_deref())
            .and_then(|e| e.background_images.first())
            .unwrap_or(l_grad);
        let l_stops = l_grad.resolved_stops(l_node.size);
        let d_stops = d_grad.resolved_stops(l_node.size);
        let stops = l_stops
            .iter()
            .enumerate()
            .map(|(i, &(lc, pos))| {
                let dc = d_stops.get(i).map_or(lc, |&(c, _)| c);
                WidgetGradientStop {
                    color: WidgetColor::from_packed(lc, dc),
                    position: pos.0,
                }
            })
            .collect();
        return Some(WidgetFill::LinearGradient {
            angle_deg: angle.0.to_degrees(),
            stops,
        });
    }
    extract_solid_bg(light, dark).map(|color| WidgetFill::Solid { color })
}

fn extract_box_style(
    light: Option<&CapturedNode>,
    dark: Option<&CapturedNode>,
    hints: Option<&crate::widget::WidgetHints>,
) -> WidgetBoxStyle {
    let background = extract_fill(light, dark);
    let l_vs = light.map(|c| &c.view_style);
    let d_vs = dark.map(|c| &c.view_style);

    let border_width = l_vs
        .map(|vs| {
            let w = &vs.borders.widths;
            w.top.0.max(w.right.0).max(w.bottom.0).max(w.left.0)
        })
        .unwrap_or(0.0);
    let border_color = if border_width > 0.0 {
        let pick_border = |vs: &ViewStyle| {
            let w = &vs.borders.widths;
            let sides = [w.top.0, w.right.0, w.bottom.0, w.left.0];
            sides
                .iter()
                .enumerate()
                .find(|&(_, &width)| width > 0.0)
                .map(|(i, _)| vs.borders.colors[i])
        };
        pair_option_color(l_vs.and_then(pick_border), d_vs.and_then(pick_border))
    } else {
        None
    };

    let corner_radius = l_vs
        .map(|vs| {
            let c = vs.corner_radius;
            c.top_left
                .or(c.top_right)
                .or(c.bottom_left)
                .or(c.bottom_right)
                .unwrap_or(vs.border_radius)
        })
        .unwrap_or(Points(0.0));

    let opacity = l_vs.map_or(1.0, |vs| vs.opacity);
    let clip = l_vs.is_some_and(|vs| vs.overflow_hidden || corner_radius.0 > 0.0);
    let accentable = hints.and_then(|h| h.accentable).unwrap_or_else(
        || matches!(&background, Some(WidgetFill::Solid { color }) if color.is_chromatic()),
    );

    WidgetBoxStyle {
        background,
        border_color,
        border_width: Points(border_width),
        corner_radius,
        opacity,
        clip,
        accentable,
    }
}

fn map_text_align(align: TextAlign) -> WidgetTextAlign {
    match align {
        TextAlign::Center => WidgetTextAlign::Center,
        TextAlign::Right => WidgetTextAlign::Trailing,
        TextAlign::Auto | TextAlign::Left | TextAlign::Justify => WidgetTextAlign::Leading,
    }
}

fn map_resize_mode(mode: ResizeMode) -> WidgetResizeMode {
    match mode {
        ResizeMode::Cover => WidgetResizeMode::Cover,
        ResizeMode::Stretch => WidgetResizeMode::Stretch,
        ResizeMode::Center => WidgetResizeMode::Center,
        ResizeMode::Contain | ResizeMode::ScaleDown => WidgetResizeMode::Contain,
    }
}

fn map_image_source(uri: &str) -> WidgetImageSource {
    if let Some(symbol) = uri.strip_prefix("symbol:") {
        WidgetImageSource::Symbol {
            name: symbol.to_owned(),
        }
    } else if let Some(asset) = uri
        .strip_prefix("asset:/")
        .or_else(|| uri.strip_prefix("asset:"))
    {
        WidgetImageSource::Asset {
            path: asset.trim_start_matches('/').to_owned(),
        }
    } else if let Some(file) = uri.strip_prefix("file://") {
        WidgetImageSource::File {
            path: file.to_owned(),
        }
    } else {
        WidgetImageSource::Asset {
            path: uri.trim_start_matches('/').to_owned(),
        }
    }
}
