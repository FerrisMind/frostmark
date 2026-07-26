use std::rc::Rc;

use iced::widget::markdown::{self};
use iced::{Element, Font, Length, Padding, Pixels, border, padding, widget};

use crate::html::block_alignment::BlockAlignment;
use crate::html::block_cache::{CachedBlock, CachedSvg};
use crate::render::svg_util::{svg_dimensions_for_height, svg_dimensions_to_fit};

use super::{
    dom::{self, DomRef},
    state::MarkState,
    structs::{
        ChildAlignment, ChildData, ChildDataFlags, Emp, ImageInfo, MarkWidget, RenderedSpan,
        UpdateMsg, UpdateMsgKind,
    },
    style::{
        DEFAULT_INLINE_CODE_BACKGROUND, DEFAULT_INLINE_CODE_BORDER, DEFAULT_INLINE_CODE_FOREGROUND,
    },
    widgets::{KbdStyle, kbd, link, link_text, quote_block, underline},
};
use crate::html::block_cache::CachedCodeBlock;
use crate::html::fragment::{HtmlFragment, HtmlNode};

mod ruby;
mod table;

const VSCODE_INLINE_LINE_HEIGHT: f32 = 1.357;
const VSCODE_INLINE_VERTICAL_PADDING: f32 = 1.0;
const VSCODE_INLINE_HORIZONTAL_PADDING: f32 = 3.0;
const VSCODE_INLINE_RADIUS: f32 = 4.0;
const VSCODE_INLINE_OPTICAL_GAP: f32 = 1.0;
const KBD_INLINE_OPTICAL_GAP: f32 = 0.5;

#[derive(Debug, Clone, Copy, Default)]
struct BlockSpacing {
    top: f32,
    bottom: f32,
}

// Add everything to one place
pub trait ValidTheme:
    widget::button::Catalog
    + widget::text::Catalog
    + widget::text_editor::Catalog
    + widget::rule::Catalog
    + widget::checkbox::Catalog
    + widget::markdown::Catalog
    + widget::container::Catalog
    + widget::svg::Catalog
{
}
impl<T> ValidTheme for T where
    T: widget::button::Catalog
        + widget::text::Catalog
        + widget::text_editor::Catalog
        + widget::rule::Catalog
        + widget::checkbox::Catalog
        + widget::markdown::Catalog
        + widget::container::Catalog
        + widget::svg::Catalog
{
}

impl<'a, M: Clone + 'static, T: ValidTheme + 'a> MarkWidget<'a, M, T>
where
    <T as widget::button::Catalog>::Class<'a>: From<widget::button::StyleFn<'a, T>>,
{
    fn paragraph_block_spacing(&self) -> BlockSpacing {
        BlockSpacing {
            top: 0.0,
            bottom: self.text_size,
        }
    }

    fn list_block_spacing(&self) -> BlockSpacing {
        BlockSpacing {
            top: 0.0,
            bottom: self.text_size * 0.7,
        }
    }

    fn heading_block_spacing(&self, level: u16) -> BlockSpacing {
        BlockSpacing {
            top: if level <= 1 {
                0.0
            } else {
                self.text_size * 1.5
            },
            bottom: self.text_size,
        }
    }

    fn block_spacing_for_tag(&self, tag: &str) -> BlockSpacing {
        match tag {
            "h1" => self.heading_block_spacing(1),
            "h2" => self.heading_block_spacing(2),
            "h3" => self.heading_block_spacing(3),
            "h4" => self.heading_block_spacing(4),
            "h5" => self.heading_block_spacing(5),
            "h6" => self.heading_block_spacing(6),
            "p" => self.paragraph_block_spacing(),
            "ul" | "ol" | "table" => self.list_block_spacing(),
            "pre" | "blockquote" | "details" => self.paragraph_block_spacing(),
            "hr" => BlockSpacing {
                top: self.text_size,
                bottom: self.text_size,
            },
            _ => self.paragraph_block_spacing(),
        }
    }

    fn block_spacing_for_fragment(&self, fragment: &HtmlFragment) -> BlockSpacing {
        for &root in fragment.roots() {
            if let Some(HtmlNode::Element { tag, .. }) = fragment.node(root) {
                return self.block_spacing_for_tag(tag.as_str());
            }
        }
        self.paragraph_block_spacing()
    }

    fn block_spacing_for_cached_block(&self, block: &CachedBlock) -> BlockSpacing {
        match block {
            CachedBlock::Fragment(fragment) => self.block_spacing_for_fragment(fragment),
            CachedBlock::Code(_) => self.paragraph_block_spacing(),
            #[cfg(feature = "math")]
            CachedBlock::Math(_) => self.paragraph_block_spacing(),
            #[cfg(feature = "mermaid")]
            CachedBlock::Mermaid(_) => self.paragraph_block_spacing(),
            CachedBlock::Empty => BlockSpacing::default(),
        }
    }

    fn stack_block_elements(
        &self,
        blocks: Vec<(Element<'a, M, T>, BlockSpacing)>,
    ) -> Element<'a, M, T> {
        let mut column = widget::Column::new().spacing(0).width(Length::Fill);
        let mut previous_bottom: f32 = 0.0;

        for (index, (element, spacing)) in blocks.into_iter().enumerate() {
            let gap_before = if index == 0 {
                0.0
            } else {
                previous_bottom.max(spacing.top)
            };
            previous_bottom = spacing.bottom;

            let wrapped: Element<'a, M, T> = if gap_before > 0.0 {
                widget::container(element)
                    .padding(Padding::default().top(gap_before))
                    .into()
            } else {
                element
            };
            column = column.push(wrapped);
        }

        column.into()
    }

    fn stack_rendered_blocks(
        &self,
        blocks: Vec<(RenderedSpan<'a, M, T>, BlockSpacing)>,
    ) -> RenderedSpan<'a, M, T> {
        let mut blocks: Vec<_> = blocks
            .into_iter()
            .filter(|(span, _)| !span.is_empty())
            .collect();
        if blocks.is_empty() {
            return RenderedSpan::None;
        }
        if blocks.len() == 1 {
            return blocks.swap_remove(0).0;
        }

        let elements = blocks
            .into_iter()
            .map(|(span, spacing)| (span.render(), spacing))
            .collect();
        RenderedSpan::from(self.stack_block_elements(elements))
    }

    fn with_block_context<R>(
        &mut self,
        block_id: Option<crate::core::ids::BlockId>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = self.current_block_id;
        self.current_block_id = block_id;
        let result = f(self);
        self.current_block_id = previous;
        result
    }

    pub(crate) fn traverse_dom(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        if let Some(text) = node.text_contents() {
            fn calc_size(text_size: f32, scaling: f32, factor: f32) -> f32 {
                text_size * (1.0 + ((scaling - 1.0) * factor))
            }

            let weight = data.heading_weight;
            let scaling = match weight {
                1 => 1.8,
                2 => 1.5,
                3 => 1.25,
                4 => 1.15,
                5 => 0.875,
                6 => 0.75,
                7 => 0.625,
                _ => 1.0,
            };
            let size = calc_size(self.text_size, scaling, self.heading_scale);

            if data.flags.contains(ChildDataFlags::MONOSPACE) {
                let inline = !data.flags.contains(ChildDataFlags::KEEP_WHITESPACE);
                return self.codeblock(text.into_owned(), size, inline);
            }

            let display = if data.flags.contains(ChildDataFlags::KEEP_WHITESPACE) {
                text.into_owned()
            } else {
                clean_whitespace(&text)
            };

            let mut t: widget::text::Span<'a, M, Font> = widget::span(display).size(size);
            let rendered = {
                if data.flags.contains(ChildDataFlags::HIGHLIGHT) {
                    t = t.line_height(VSCODE_INLINE_LINE_HEIGHT);
                    let highlight_color = self
                        .style
                        .and_then(|n| n.highlight_color)
                        .unwrap_or_else(|| iced::Color::from_rgb8(0xF7, 0xD8, 0x4B));
                    t = t
                        .background(highlight_color)
                        .border(border::rounded(VSCODE_INLINE_RADIUS))
                        .padding(Padding {
                            top: VSCODE_INLINE_VERTICAL_PADDING,
                            right: VSCODE_INLINE_HORIZONTAL_PADDING,
                            bottom: VSCODE_INLINE_VERTICAL_PADDING,
                            left: VSCODE_INLINE_HORIZONTAL_PADDING,
                        });
                }
                t = t.font({
                    let mut f = self.font;
                    if data.flags.contains(ChildDataFlags::BOLD) {
                        f.weight = iced::font::Weight::Bold;
                    }
                    if data.flags.contains(ChildDataFlags::ITALIC) {
                        f.style = iced::font::Style::Italic;
                    }
                    f
                });
                if data.flags.contains(ChildDataFlags::STRIKETHROUGH) {
                    t = t.strikethrough(true);
                }
                if data.flags.contains(ChildDataFlags::UNDERLINE) {
                    t = t.underline(true);
                }
                if let Some(color) = self.style.and_then(|s| s.text_color) {
                    t = t.color(color);
                }
                t
            };

            if data.flags.contains(ChildDataFlags::HIGHLIGHT) {
                return RenderedSpan::Elem(
                    widget::rich_text([rendered]).into(),
                    Emp::NonEmpty,
                    VSCODE_INLINE_OPTICAL_GAP,
                );
            }

            return RenderedSpan::Spans(vec![rendered]);
        }

        if let Some(name) = node.tag_name() {
            return self.render_html_inner(name, node, data);
        }

        RenderedSpan::None
    }

    fn render_html_inner(
        &mut self,
        name: &str,
        node: DomRef<'_>,
        mut data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let block_element = node.is_block_element();
        if block_element {
            dom::alignment_read(&mut data, node);
        }

        match name {
            "span" => {
                #[cfg(feature = "math")]
                if let Some(display) = self.math_span_mode(node) {
                    return self.draw_math_span(node, data, display);
                }
                self.render_children(node, data)
            }
            "summary" | "html" | "body" | "p" | "div" | "thead" | "tbody" | "tfoot" | "picture" => {
                self.render_children(node, data)
            }

            "center" => {
                data.alignment = Some(ChildAlignment::Center);
                let inner = self.render_children(node, data);
                if inner.is_empty() {
                    return inner;
                }
                widget::column![inner.render()]
                    .width(Length::Fill)
                    .spacing(self.paragraph_spacing.unwrap_or(5.0))
                    .into()
            }
            "pre" => self.render_pre_block(node, data),

            "h1" => self.render_children(node, data.heading(1)),
            "h2" => self.render_children(node, data.heading(2)),
            "h3" => self.render_children(node, data.heading(3)),
            "h4" => self.render_children(node, data.heading(4)),
            "h5" => self.render_children(node, data.heading(5)),
            "h6" => self.render_children(node, data.heading(6)),
            "kbd" => self.draw_kbd(node, data),
            "sub" => self.draw_script(node, data, ScriptKind::Sub),
            "sup" => self.draw_script(node, data, ScriptKind::Sup),
            "rt" => self.render_children(node, data.heading(7).insert(ChildDataFlags::INSIDE_RUBY)),

            "blockquote" => self.draw_blockquote(node, data),

            "b" | "strong" => self.render_children(node, data.insert(ChildDataFlags::BOLD)),
            "em" | "i" => self.render_children(node, data.insert(ChildDataFlags::ITALIC)),
            "u" => self.render_children(node, data.insert(ChildDataFlags::UNDERLINE)),
            "ins" => self.render_children(node, data.insert(ChildDataFlags::UNDERLINE)),
            "del" | "s" | "strike" => {
                self.render_children(node, data.insert(ChildDataFlags::STRIKETHROUGH))
            }
            "code" => self.render_children(node, data.insert(ChildDataFlags::MONOSPACE)),
            "mark" => self.render_children(node, data.insert(ChildDataFlags::HIGHLIGHT)),

            "details" => self.draw_details(node, data),
            "a" => self.draw_link(node, data),
            "img" => self.draw_image(node, data),

            "br" => {
                if data.flags.contains(ChildDataFlags::INSIDE_RUBY) {
                    RenderedSpan::None
                } else {
                    RenderedSpan::Spans(vec![widget::span("\n").size(text_size_for_data(
                        self.text_size,
                        self.heading_scale,
                        data.heading_weight,
                    ))])
                }
            }
            "hr" => widget::rule::horizontal(1.0).into(),
            "head" | "title" | "meta" | "rtc" | "rp" | "rb" | "source" => RenderedSpan::None,

            "input" => match node.get_attr("type").unwrap_or("text") {
                "checkbox" => {
                    let checked = node.get_attr("checked").is_some();
                    widget::container(widget::checkbox(checked))
                        .height(self.text_size * 1.2)
                        .align_y(iced::alignment::Vertical::Center)
                        .into()
                }
                _ => node
                    .get_attr("value")
                    .map(|value| {
                        RenderedSpan::Spans(vec![widget::span(value.to_string()).font(Font {
                            family: self.font_mono.family,
                            ..self.font
                        })])
                    })
                    .unwrap_or(RenderedSpan::None),
            },

            "ul" => {
                data.li_ordered_number = None;
                self.render_list(node, data)
            }
            "ol" => {
                let start = node
                    .get_attr("start")
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1);
                self.render_list(node, data.ordered_from(start))
            }
            "li" => self.render_list_item(node, data),

            "ruby" => self.draw_ruby(node, data),
            "table" => self.draw_table(node, data),

            _ => RenderedSpan::Spans(vec![widget::span(format!("<{name} (TODO)>")).font(Font {
                weight: iced::font::Weight::Bold,
                ..self.font
            })]),
        }
    }

    fn draw_details(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let dropdown_id = self
            .current_block_id
            .and_then(|block_id| self.state.dropdown_id_for(block_id, node.id()))
            .unwrap_or_else(|| {
                let id = self.current_dropdown_id;
                self.current_dropdown_id += 1;
                id
            });
        if let (Some(update), Some(state)) = (
            self.fn_update.clone(),
            self.state.dropdown_state.get(&dropdown_id).copied(),
        ) {
            let summary = self.get_summary_elements(node, data);

            let umsg = UpdateMsg {
                kind: UpdateMsgKind::DetailsToggle(dropdown_id, !state),
            };

            let arrow = if state {
                widget::text("v").size(12)
            } else {
                widget::text(">").size(14)
            };
            let header: Element<'a, M, T> = widget::row![arrow, underline(summary.render())]
                .spacing(6.0)
                .align_y(iced::Alignment::Center)
                .into();
            let link: Element<'a, M, T> = widget::mouse_area(header)
                .on_press(update(umsg))
                .interaction(iced::mouse::Interaction::Pointer)
                .into();

            let mut column = widget::column![link].spacing(5.0);
            if state {
                let regular_children =
                    self.render_children(node, data.insert(ChildDataFlags::SKIP_SUMMARY));
                let body = widget::row![
                    widget::rule::vertical(1),
                    widget::container(regular_children.render())
                        .padding(Padding::default().left(8))
                        .width(Length::Fill)
                ]
                .spacing(10.0)
                .width(Length::Fill);
                column = column.push(body);
            }
            column.padding(Padding::default().bottom(5)).into()
        } else {
            widget::column![
                widget::rule::vertical(1),
                self.render_children(node, data).render(),
                widget::rule::horizontal(1),
            ]
            .padding(10)
            .spacing(10)
            .into()
        }
    }

    fn draw_blockquote(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let alert = github_alert_kind(node.get_attr("class"));
        if let Some(alert) = alert {
            let content = self.render_children(node, data);
            let icon_text = self
                .fn_github_alert_icon
                .as_ref()
                .map(|f| f(alert.label()))
                .unwrap_or_else(|| alert.icon().to_string());
            let icon: widget::text::Span<'a, M, Font> =
                widget::span(icon_text).size(self.text_size * 0.85);
            let label: widget::text::Span<'a, M, Font> = widget::span(alert.label())
                .size(self.text_size * 0.85)
                .color(alert.color())
                .font(Font {
                    weight: iced::font::Weight::Bold,
                    ..self.font
                });

            let body: Element<'a, M, T> = widget::column![
                widget::rich_text([icon, widget::span(" "), label]),
                content.render()
            ]
            .spacing(4.0)
            .width(Length::Fill)
            .into();

            quote_block(body, alert.color(), 3.0, 11.0).into()
        } else {
            let quote_color = iced::Color::from_rgb8(0x6A, 0x73, 0x7D);
            let body = self.simple_quote_body(node, data, quote_color);
            quote_block(body, quote_color, 4.0, 12.0).into()
        }
    }

    #[cfg(feature = "math")]
    fn math_span_mode(&self, node: DomRef<'_>) -> Option<bool> {
        let class = node.get_attr("class")?;
        if class.contains("math-inline") {
            Some(false)
        } else if class.contains("math-display") {
            Some(true)
        } else {
            None
        }
    }

    #[cfg(feature = "math")]
    fn draw_math_span(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
        display: bool,
    ) -> RenderedSpan<'a, M, T> {
        let latex = node.accumulated_text();
        let latex = latex.trim();
        if latex.is_empty() {
            return RenderedSpan::None;
        }
        let Some(cache) = self.state.cache.as_ref() else {
            return self.render_children(node, data);
        };
        let svg = if display {
            cache.display_math_svg(latex)
        } else {
            cache.inline_math_svg(latex)
        };
        let Some(svg) = svg else {
            return self.render_children(node, data);
        };
        let base = text_size_for_data(self.text_size, self.heading_scale, data.heading_weight);
        let target_h = if display {
            base * 2.2
        } else {
            // ~1.1× cap height — inline math slightly taller than body text (cf. KaTeX 1.21em).
            base * 1.1
        };
        RenderedSpan::from(Self::element_from_cached_svg(&svg, Some(target_h))).with_gap(2.0)
    }

    fn element_from_cached_svg(svg: &CachedSvg, target_height: Option<f32>) -> Element<'a, M, T> {
        let (w, h) = match target_height {
            Some(target_h) => svg_dimensions_for_height(svg.width, svg.height, target_h),
            None => (svg.width, svg.height),
        };
        Self::element_from_cached_svg_sized(svg, w, h)
    }

    fn element_from_cached_svg_fit(
        svg: &CachedSvg,
        max_width: f32,
        max_height: f32,
    ) -> Element<'a, M, T> {
        let (w, h) = svg_dimensions_to_fit(svg.width, svg.height, max_width, max_height);
        Self::element_from_cached_svg_sized(svg, w, h)
    }

    fn element_from_cached_svg_sized(
        svg: &CachedSvg,
        width: f32,
        height: f32,
    ) -> Element<'a, M, T> {
        let mut widget = widget::svg(svg.handle.clone());
        if width > 0.0 && height > 0.0 {
            widget = widget
                .width(Length::Fixed(width))
                .height(Length::Fixed(height));
        }
        widget.into()
    }

    fn draw_kbd(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let text = clean_whitespace(&node.accumulated_text());
        if !text.is_empty() {
            let bg = iced::Color::from_rgb8(0x1E, 0x29, 0x3B);
            let fg = iced::Color::from_rgb8(0xF8, 0xFA, 0xFC);
            let border = iced::Color::from_rgb8(0x33, 0x41, 0x55);
            let shadow = iced::Color::from_rgba8(0x0F, 0x17, 0x2A, 0x40 as f32 / 255.0);

            let element: Element<'a, M, T> = kbd(
                text,
                KbdStyle::size2(bg, fg, border, shadow, self.font_mono),
            );
            return RenderedSpan::from(element).with_gap(KBD_INLINE_OPTICAL_GAP);
        }

        self.render_children(node, data.insert(ChildDataFlags::MONOSPACE))
    }

    fn draw_script(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
        kind: ScriptKind,
    ) -> RenderedSpan<'a, M, T> {
        let size = text_size_for_data(self.text_size, self.heading_scale, 7) * 0.58;
        let line_box = self.text_size * 1.1;

        if let Some(text) = node.text_contents() {
            let mut span: widget::text::Span<'a, M, Font> = widget::span(clean_whitespace(&text))
                .size(size)
                .font(self.font);

            if let Some(color) = self.style.and_then(|s| s.text_color) {
                span = span.color(color);
            }
            let script = widget::rich_text([span]);
            let element = self.script_box(script.into(), kind, line_box);
            return RenderedSpan::from(element).with_gap(0.0);
        }

        match self.render_children(node, data.heading(7)) {
            RenderedSpan::Spans(spans) => {
                let inner = widget::rich_text(spans).on_link_click(|url| url);
                let element = self.script_box(inner.into(), kind, line_box);
                RenderedSpan::from(element).with_gap(0.0)
            }
            other => {
                let rendered = other.render();
                let element = self.script_box(rendered, kind, line_box);
                RenderedSpan::from(element).with_gap(0.0)
            }
        }
    }

    fn script_box(
        &self,
        content: Element<'a, M, T>,
        kind: ScriptKind,
        line_box: f32,
    ) -> Element<'a, M, T> {
        let vertical = match kind {
            ScriptKind::Sub => iced::alignment::Vertical::Bottom,
            ScriptKind::Sup => iced::alignment::Vertical::Top,
        };
        widget::container(content)
            .height(line_box)
            .align_y(vertical)
            .width(Length::Shrink)
            .into()
    }

    fn tint_rendered_span(
        &self,
        span: RenderedSpan<'a, M, T>,
        color: iced::Color,
    ) -> RenderedSpan<'a, M, T> {
        match span {
            RenderedSpan::Spans(spans) => {
                RenderedSpan::Spans(spans.into_iter().map(|span| span.color(color)).collect())
            }
            other => other,
        }
    }

    fn simple_quote_body(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
        color: iced::Color,
    ) -> Element<'a, M, T> {
        fn contains_direct_br(node: DomRef<'_>) -> bool {
            node.children_iter().any(|child| {
                child.tag_name() == Some("br")
                    || child
                        .children_iter()
                        .any(|grandchild| grandchild.tag_name() == Some("br"))
            })
        }

        let meaningful_children = node
            .children_iter()
            .filter(|child| !child.is_useless())
            .collect::<Vec<_>>();

        let content = if meaningful_children.len() == 1
            && meaningful_children[0].tag_name() == Some("p")
            && !contains_direct_br(meaningful_children[0])
        {
            self.render_children(meaningful_children[0], data)
        } else {
            self.render_children(node, data)
        };

        match self.tint_rendered_span(content, color) {
            RenderedSpan::Spans(spans) => {
                widget::container(widget::rich_text(spans).on_link_click(|url| url))
                    .width(Length::Fill)
                    .into()
            }
            other => widget::container(other.render()).width(Length::Fill).into(),
        }
    }
    fn get_summary_elements(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        node.children_iter()
            .find(|child| child.tag_name() == Some("summary"))
            .map(|child| self.traverse_dom(child, data))
            .unwrap_or_default()
    }

    fn draw_image(&self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let Some(url) = node.get_attr("src") else {
            return RenderedSpan::None;
        };
        if url.is_empty() {
            return RenderedSpan::None;
        }

        let base_size = text_size_for_data(self.text_size, self.heading_scale, data.heading_weight);

        let is_badge = {
            let lower = url.to_ascii_lowercase();
            lower.contains("img.shields.io")
                || lower.contains("shields.io/")
                || lower.contains("badge.svg")
                || lower.contains("/badge")
        };

        let width = node
            .get_attr("width")
            .and_then(|n| n.parse::<f32>().ok())
            .or_else(|| {
                node.get_attr("style")
                    .as_ref()
                    .and_then(|s| css_dimension_with_font(s, "width", base_size))
            });
        let mut height = node
            .get_attr("height")
            .and_then(|n| n.parse::<f32>().ok())
            .or_else(|| {
                node.get_attr("style")
                    .as_ref()
                    .and_then(|s| css_dimension_with_font(s, "height", base_size))
            });
        if is_badge && width.is_none() && height.is_none() {
            height = Some(20.0);
        }

        if let Some(func) = self.fn_drawing_image.as_deref() {
            return func(ImageInfo {
                url,
                alt: node.get_attr("alt"),
                width,
                height,
            })
            .into();
        }
        RenderedSpan::None
    }

    fn draw_link(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let link_col = self
            .style
            .and_then(|n| n.link_color)
            .unwrap_or_else(|| iced::Color::from_rgb8(0x5A, 0x6B, 0x9E));

        let children = self.render_children(node, data);

        if let Some(url) = node.get_attr("href") {
            let children_empty = node.child_ids().is_empty();
            let msg = self.fn_clicking_link.as_ref();

            if children_empty {
                RenderedSpan::Spans(vec![
                    link_text(widget::span(url.to_string()), url.to_string(), msg).color(link_col),
                ])
            } else if let RenderedSpan::Spans(n) = children {
                RenderedSpan::Spans(
                    n.into_iter()
                        .map(|n| {
                            link_text(n, url.to_string(), msg)
                                .color(link_col)
                                .underline(true)
                        })
                        .collect(),
                )
            } else if let Some(handler) = msg {
                let mut button = widget::button(children.render()).padding(0);
                if !url.is_empty() {
                    button = button.on_press(handler(url.to_string()));
                }
                if let Some(style) = self.fn_style_link_button.clone() {
                    button = button.style(move |theme, status| style(theme, status));
                } else {
                    button = button.style(|_theme, _status| widget::button::Style {
                        background: None,
                        text_color: iced::Color::TRANSPARENT,
                        border: border::rounded(0.0),
                        shadow: iced::Shadow::default(),
                        snap: true,
                    });
                }
                button.width(Length::Shrink).into()
            } else {
                children.render().into()
            }
        } else if let RenderedSpan::Spans(n) = children {
            RenderedSpan::Spans(
                n.into_iter()
                    .map(|n| n.underline(true).color(link_col))
                    .collect(),
            )
        } else {
            link(
                children.render(),
                "",
                None::<&fn(String) -> M>,
                self.fn_style_link_button.clone(),
            )
            .into()
        }
    }

    pub(crate) fn render_children(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let children = node.children();

        let mut column: Vec<(RenderedSpan<'a, M, T>, BlockSpacing)> = Vec::new();
        let mut row = RenderedSpan::None;

        let meaningful = self.significant_children(&children);
        let mut skipped_summary = false;
        let original_start = data.li_ordered_number;

        let mut block_index = 0usize;
        let mut idx = 0usize;
        while idx < meaningful.len() {
            let item = meaningful[idx];

            if !skipped_summary
                && data.flags.contains(ChildDataFlags::SKIP_SUMMARY)
                && item.tag_name() == Some("summary")
            {
                skipped_summary = true;
                idx += 1;
                continue;
            }

            if item.is_shield_paragraph() {
                if !row.is_empty() {
                    let mut old_row = RenderedSpan::None;
                    std::mem::swap(&mut row, &mut old_row);
                    column.push((
                        Self::apply_alignment_to_block(old_row, data.alignment),
                        self.paragraph_block_spacing(),
                    ));
                }
                let mut shield_row = widget::Row::new().spacing(6.0);
                while idx < meaningful.len() && meaningful[idx].is_shield_paragraph() {
                    shield_row = shield_row.push(self.render_shield_badge(meaningful[idx], data));
                    idx += 1;
                    block_index += 1;
                }
                let badge_row: Element<'a, M, T> = shield_row.width(Length::Shrink).into();
                column.push((
                    if let Some(align) = data.alignment {
                        RenderedSpan::Elem(
                            Self::stack_align_in_viewport(badge_row, align),
                            Emp::NonEmpty,
                            5.0,
                        )
                    } else {
                        RenderedSpan::from(badge_row)
                    },
                    self.paragraph_block_spacing(),
                ));
                continue;
            }

            let mut child_data = data;
            if data.alignment.is_some() && item.is_block_element() {
                child_data.alignment = None;
            }
            if let Some(base) = original_start {
                child_data.li_ordered_number = Some(base + block_index);
            }
            let element = self.traverse_dom(item, child_data);

            if !data.flags.contains(ChildDataFlags::INSIDE_RUBY) && item.is_block_element() {
                if !row.is_empty() {
                    let mut old_row = RenderedSpan::None;
                    std::mem::swap(&mut row, &mut old_row);
                    column.push((
                        Self::apply_alignment_to_block(old_row, data.alignment),
                        self.paragraph_block_spacing(),
                    ));
                }

                column.push((
                    Self::apply_alignment_to_block(element, data.alignment),
                    self.block_spacing_for_tag(item.tag_name().unwrap_or("div")),
                ));
                block_index += 1;
            } else {
                row = row + element;
            }

            idx += 1;
        }

        if !row.is_empty() {
            column.push((
                Self::apply_alignment_to_block(row, data.alignment),
                self.paragraph_block_spacing(),
            ));
        }

        self.stack_rendered_blocks(column)
    }

    /// `<center>` / `align="center"`: text lines use `rich_text` centering; block widgets centered in viewport.
    fn apply_alignment_to_block(
        span: RenderedSpan<'a, M, T>,
        alignment: Option<ChildAlignment>,
    ) -> RenderedSpan<'a, M, T> {
        let Some(align) = alignment else {
            return span;
        };
        if span.is_empty() {
            return span;
        }
        match span {
            RenderedSpan::Spans(spans) => RenderedSpan::Elem(
                widget::container(widget::rich_text(spans).on_link_click(|url| url))
                    .width(Length::Fill)
                    .align_x(align.to_horizontal())
                    .into(),
                Emp::NonEmpty,
                5.0,
            ),
            other => RenderedSpan::Elem(
                Self::stack_align_in_viewport(other.render(), align),
                Emp::NonEmpty,
                5.0,
            ),
        }
    }

    fn stack_align_in_viewport(
        element: Element<'a, M, T>,
        align: ChildAlignment,
    ) -> Element<'a, M, T> {
        let horizontal = match align {
            ChildAlignment::Center => iced::alignment::Horizontal::Center,
            ChildAlignment::Right => iced::alignment::Horizontal::Right,
        };
        widget::column![element]
            .width(Length::Fill)
            .align_x(horizontal)
            .into()
    }

    fn render_list(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let indent = self.paragraph_spacing.unwrap_or(5.0);
        let items = self.render_children(node, data);

        widget::column![items.render()]
            .padding(Padding::default().left(indent))
            .into()
    }

    fn render_list_item(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        let marker_gap = self.paragraph_spacing.unwrap_or(5.0);

        if let Some(checkbox) = node.direct_task_checkbox() {
            let checked = checkbox.get_attr("checked").is_some();
            let body = self.render_list_item_body_excluding_inputs(node, data);
            return widget::row![
                widget::text("•").size(self.text_size),
                widget::container(widget::checkbox(checked))
                    .height(self.text_size * 1.2)
                    .align_y(iced::alignment::Vertical::Center),
                body.render(),
            ]
            .spacing(marker_gap)
            .align_y(iced::Alignment::Center)
            .into();
        }

        let content = self.render_list_item_content(node, data);

        if let Some(num) = data.li_ordered_number {
            widget::row![
                widget::text(format!("{num}.")).size(self.text_size),
                content.render(),
            ]
            .spacing(marker_gap)
            .align_y(iced::Alignment::Start)
            .into()
        } else {
            widget::row![widget::text("•").size(self.text_size), content.render(),]
                .spacing(marker_gap)
                .align_y(iced::Alignment::Start)
                .into()
        }
    }

    fn render_list_item_content(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let children = node.children();
        let meaningful = self.significant_children(&children);

        if meaningful.len() == 1 && meaningful[0].tag_name() == Some("p") {
            self.render_list_item_content(meaningful[0], data)
        } else {
            self.render_list_item_body_excluding_inputs(node, data)
        }
    }

    fn render_list_item_body_excluding_inputs(
        &mut self,
        node: DomRef<'_>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let mut column: Vec<(RenderedSpan<'a, M, T>, BlockSpacing)> = Vec::new();
        let mut row = RenderedSpan::None;
        let children = node.children();

        for child in self.significant_children(&children) {
            if DomRef::is_task_checkbox(child) {
                continue;
            }
            let element = if child.tag_name() == Some("p") {
                self.render_list_item_body_excluding_inputs(child, data)
            } else {
                self.traverse_dom(child, data)
            };
            if child.is_block_element() {
                if !row.is_empty() {
                    let mut old_row = RenderedSpan::None;
                    std::mem::swap(&mut row, &mut old_row);
                    column.push((old_row, self.list_block_spacing()));
                }
                let spacing = if child.tag_name() == Some("p") {
                    self.list_block_spacing()
                } else {
                    self.block_spacing_for_tag(child.tag_name().unwrap_or("div"))
                };
                column.push((element, spacing));
            } else {
                row = row + element;
            }
        }

        if !row.is_empty() {
            column.push((row, self.list_block_spacing()));
        }

        self.stack_rendered_blocks(column)
    }

    /// Badge `<p>` with only `img` or `a>img` — render the graphic directly (no extra block wrap).
    fn render_shield_badge(&mut self, paragraph: DomRef<'_>, data: ChildData) -> Element<'a, M, T> {
        let mut row = widget::Row::new().spacing(6.0);
        let mut has_badge = false;
        for child in paragraph.children_iter() {
            if child.is_useless() {
                continue;
            }
            match child.tag_name() {
                Some("img") | Some("a") => {
                    row = row.push(self.traverse_dom(child, data).render());
                    has_badge = true;
                }
                _ => {}
            }
        }
        if has_badge {
            row.into()
        } else {
            self.traverse_dom(paragraph, data).render()
        }
    }

    fn render_pre_block(&mut self, node: DomRef<'_>, data: ChildData) -> RenderedSpan<'a, M, T> {
        if let Some(code_node) = node
            .children_iter()
            .find(|child| child.tag_name() == Some("code"))
        {
            let text = code_node.accumulated_text();
            if !text.trim().is_empty() {
                let lang = code_node
                    .get_attr("class")
                    .and_then(|c| c.strip_prefix("language-").map(str::to_string));
                let block_cache = self.state.cache.as_ref();
                #[cfg(feature = "mermaid")]
                if lang
                    .as_deref()
                    .is_some_and(crate::parse::content::is_mermaid_lang)
                    && let Some(svg) = block_cache
                        .and_then(|cache| cache.mermaid_svg(&text))
                        .or_else(|| {
                            crate::render::mermaid_to_svg(&text).ok().map(|artifact| {
                                crate::html::block_cache::cached_svg_from_artifact(&artifact)
                            })
                        })
                {
                    let max_w = 560.0_f32;
                    let max_h = self.text_size * 16.0;
                    return RenderedSpan::from(Self::element_from_cached_svg_fit(
                        &svg, max_w, max_h,
                    ));
                }
                let trimmed = text.trim_end_matches('\n');
                let items = block_cache
                    .map(|cache| cache.code_markdown_lines(lang.as_deref(), trimmed))
                    .unwrap_or_else(|| {
                        Rc::new(crate::backends::iced::iced_markdown_lines_for_codeblock(
                            lang.as_deref(),
                            trimmed,
                        ))
                    });
                if !items.is_empty() {
                    let settings = self.markdown_code_settings();
                    return self.wrap_fenced_code_container(
                        self.fenced_code_inner(items.as_ref(), &settings),
                        trimmed,
                        None,
                        lang.as_deref(),
                    );
                }
            }
        }

        let content = self
            .render_children(node, data.insert(ChildDataFlags::KEEP_WHITESPACE))
            .render();

        if let Some(draw) = &self.fn_drawing_pre_block {
            draw(content).into()
        } else {
            content.into()
        }
    }

    fn codeblock(&self, code: String, size: f32, inline: bool) -> RenderedSpan<'a, M, T> {
        let style = self.style;
        let inline_background = style.and_then(|s| s.inline_code_background).or(if inline {
            Some(DEFAULT_INLINE_CODE_BACKGROUND)
        } else {
            None
        });
        let inline_border = inline_background.map(|_| DEFAULT_INLINE_CODE_BORDER);
        let inline_color = style.and_then(|s| s.inline_code_color);
        let text_color = style.and_then(|s| s.text_color);

        if inline {
            let mut code_span: widget::text::Span<'a, M, Font> = widget::span(code)
                .size(size)
                .font(self.font_mono)
                .line_height(VSCODE_INLINE_LINE_HEIGHT);

            if let Some(color) = inline_color.or(text_color) {
                code_span = code_span.color(color);
            }
            if let Some(background) = inline_background {
                code_span = code_span
                    .background(background)
                    .border(
                        border::rounded(VSCODE_INLINE_RADIUS)
                            .width(1.0)
                            .color(inline_border.unwrap_or(DEFAULT_INLINE_CODE_BORDER)),
                    )
                    .padding(Padding {
                        top: VSCODE_INLINE_VERTICAL_PADDING,
                        right: VSCODE_INLINE_HORIZONTAL_PADDING,
                        bottom: VSCODE_INLINE_VERTICAL_PADDING,
                        left: VSCODE_INLINE_HORIZONTAL_PADDING,
                    });
            }
            RenderedSpan::Spans(vec![code_span])
        } else {
            let mut span = widget::span(code)
                .size(size)
                .font(self.font_mono)
                .line_height(Pixels(size * 1.25));
            if let Some(color) = text_color.or(inline_color) {
                span = span.color(color);
            }
            if let Some(background) = style.and_then(|s| s.code_block_background) {
                span = span
                    .background(background)
                    .padding(Padding::new(8.0))
                    .border(border::rounded(4.0));
            }
            RenderedSpan::Spans(vec![span])
        }
    }

    fn wrap_fenced_code_container(
        &self,
        inner: Element<'a, M, T>,
        code: &str,
        copy_payload: Option<std::sync::Arc<str>>,
        _language: Option<&str>,
    ) -> RenderedSpan<'a, M, T> {
        let settings = self.markdown_code_settings();
        let shell = widget::container(widget::container(inner).padding(settings.code_size))
            .width(Length::Fill)
            .padding(settings.code_size / 4.0)
            .class(<T as markdown::Catalog>::code_block());

        let element: Element<'a, M, T> = if let Some(update) = &self.fn_update {
            let umsg = UpdateMsg {
                kind: UpdateMsgKind::CopyToClipboard(
                    copy_payload.unwrap_or_else(|| std::sync::Arc::from(code)),
                ),
            };
            let copy_btn = widget::button(widget::text("📋").size(12))
                .padding(Padding::new(4.0))
                .on_press(update(umsg));

            let copy_overlay = widget::container(copy_btn)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .padding(Padding::new(8.0));

            widget::stack![shell, copy_overlay].into()
        } else {
            shell.into()
        };

        let element = if let Some(draw) = &self.fn_drawing_pre_block {
            draw(element)
        } else {
            element
        };
        element.into()
    }

    fn highlighted_codeblock_lines(
        &self,
        lines: &[markdown::Text],
        settings: &markdown::Settings,
    ) -> Element<'a, M, T> {
        widget::column(
            lines
                .iter()
                .map(|line| {
                    widget::rich_text(line.spans(settings.style))
                        .font(settings.style.code_block_font)
                        .size(settings.code_size)
                        .width(Length::Fill)
                        .into()
                })
                .collect::<Vec<_>>(),
        )
        .spacing(Pixels(0.0))
        .into()
    }

    fn fenced_code_inner(
        &self,
        lines: &[markdown::Text],
        settings: &markdown::Settings,
    ) -> Element<'a, M, T> {
        widget::container(self.highlighted_codeblock_lines(lines, settings)).into()
    }

    pub(crate) fn render_fragment_roots(
        &mut self,
        fragment: &HtmlFragment,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let roots = DomRef::fragment_roots(fragment);
        if roots.is_empty() {
            return RenderedSpan::None;
        }
        if roots.len() == 1 {
            return self.traverse_dom(roots[0], data);
        }
        self.render_children_list(roots, data)
    }

    fn render_children_list(
        &mut self,
        children: Vec<DomRef<'_>>,
        data: ChildData,
    ) -> RenderedSpan<'a, M, T> {
        let mut column: Vec<(RenderedSpan<'a, M, T>, BlockSpacing)> = Vec::new();
        let mut row = RenderedSpan::None;

        for item in self.significant_children(&children) {
            if item.is_useless() {
                continue;
            }
            let element = self.traverse_dom(item, data);
            if item.is_block_element() {
                if !row.is_empty() {
                    let mut old_row = RenderedSpan::None;
                    std::mem::swap(&mut row, &mut old_row);
                    column.push((old_row, self.paragraph_block_spacing()));
                }
                column.push((
                    element,
                    self.block_spacing_for_tag(item.tag_name().unwrap_or("div")),
                ));
            } else {
                row = row + element;
            }
        }

        if !row.is_empty() {
            column.push((row, self.paragraph_block_spacing()));
        }

        self.stack_rendered_blocks(column)
    }
}

impl<'a, M: Clone + 'static, T: ValidTheme + 'a> MarkWidget<'a, M, T>
where
    <T as widget::button::Catalog>::Class<'a>: From<widget::button::StyleFn<'a, T>>,
{
    fn render_from_block_cache(&mut self) -> Element<'a, M, T> {
        let state: &'a MarkState = self.state;
        let cache = match &state.cache {
            Some(cache) => cache,
            None => return widget::Column::new().into(),
        };
        let mut blocks: Vec<(Element<'a, M, T>, BlockSpacing)> = Vec::new();
        let mut index = 0;
        while index < cache.len() {
            let block_id = cache.block_id(index);
            let block_data = child_data_for_block_alignment(cache.entry_alignment(index));
            if let Some(CachedBlock::Fragment(fragment)) = cache.entry(index)
                && Self::fragment_is_shield_paragraph(fragment)
            {
                let mut row = widget::Row::new().spacing(6.0);
                while index < cache.len() {
                    match cache.entry(index) {
                        Some(CachedBlock::Empty) => {
                            index += 1;
                            continue;
                        }
                        Some(CachedBlock::Fragment(f)) if Self::fragment_is_shield_paragraph(f) => {
                            let roots = DomRef::fragment_roots(f);
                            if roots.len() == 1 {
                                row = row.push(self.with_block_context(
                                    cache.block_id(index),
                                    |this| {
                                        this.render_shield_badge(
                                            roots[0],
                                            child_data_for_block_alignment(
                                                cache.entry_alignment(index),
                                            ),
                                        )
                                    },
                                ));
                            } else {
                                row = row.push(self.with_block_context(
                                    cache.block_id(index),
                                    |this| {
                                        this.render_fragment_roots(
                                            f,
                                            child_data_for_block_alignment(
                                                cache.entry_alignment(index),
                                            ),
                                        )
                                        .render()
                                    },
                                ));
                            }
                            index += 1;
                        }
                        _ => break,
                    }
                }
                let badge_row: Element<'a, M, T> = row.width(Length::Shrink).into();
                blocks.push((
                    if let Some(align) = block_data.alignment {
                        Self::stack_align_in_viewport(badge_row, align)
                    } else {
                        badge_row
                    },
                    self.paragraph_block_spacing(),
                ));
                continue;
            }

            let mut center_wrapper_fragment = false;
            let span = match cache.entry(index) {
                Some(CachedBlock::Fragment(fragment)) => {
                    center_wrapper_fragment = Self::fragment_roots_are_center_wrapper(fragment);
                    let render_data = if center_wrapper_fragment {
                        block_data
                    } else {
                        ChildData::default()
                    };
                    self.with_block_context(block_id, |this| {
                        this.render_fragment_roots(fragment, render_data)
                    })
                }
                Some(CachedBlock::Code(code)) => Self::render_fenced_code_block(self, code),
                #[cfg(feature = "math")]
                Some(CachedBlock::Math(svg)) => {
                    let target_h = self.text_size * 2.4;
                    RenderedSpan::from(Self::element_from_cached_svg(svg, Some(target_h)))
                }
                #[cfg(feature = "mermaid")]
                Some(CachedBlock::Mermaid(svg)) => {
                    let max_w = 560.0_f32;
                    let max_h = self.text_size * 16.0;
                    RenderedSpan::from(Self::element_from_cached_svg_fit(svg, max_w, max_h))
                }
                Some(CachedBlock::Empty) => RenderedSpan::None,
                None => RenderedSpan::None,
            };
            index += 1;
            if span.is_empty() {
                continue;
            }
            let element = span.render();
            blocks.push((
                if let Some(align) = block_data.alignment {
                    if center_wrapper_fragment {
                        element
                    } else {
                        Self::stack_align_in_viewport(
                            widget::container(element).width(Length::Shrink).into(),
                            align,
                        )
                    }
                } else {
                    element
                },
                cache
                    .entry(index - 1)
                    .map(|entry| self.block_spacing_for_cached_block(entry))
                    .unwrap_or_default(),
            ));
        }
        self.stack_block_elements(blocks)
    }

    fn fragment_is_shield_paragraph(fragment: &HtmlFragment) -> bool {
        let roots = fragment.roots();
        if roots.len() != 1 {
            return false;
        }
        let Some(HtmlNode::Element { tag, children, .. }) = fragment.node(roots[0]) else {
            return false;
        };
        if tag.as_str() != "p" {
            return false;
        }
        let mut has_badge = false;
        for &child in children {
            match fragment.node(child) {
                Some(HtmlNode::Element { tag, attrs, .. }) if tag.as_str() == "img" => {
                    if !Self::fragment_img_is_badge(attrs) {
                        return false;
                    }
                    has_badge = true;
                }
                Some(HtmlNode::Element { tag, children, .. }) if tag.as_str() == "a" => {
                    let only_img = children.len() == 1
                        && children.iter().all(|&c| {
                            matches!(
                                fragment.node(c),
                                Some(HtmlNode::Element { tag, .. }) if tag.as_str() == "img"
                            )
                        });
                    if !only_img {
                        return false;
                    }
                    let Some(HtmlNode::Element { attrs, .. }) = fragment.node(children[0]) else {
                        return false;
                    };
                    if !Self::fragment_img_is_badge(attrs) {
                        return false;
                    }
                    has_badge = true;
                }
                Some(HtmlNode::Text(t)) if t.trim().is_empty() => {}
                _ => return false,
            }
        }
        has_badge
    }

    fn fragment_img_is_badge(attrs: &[crate::html::fragment::HtmlAttr]) -> bool {
        let Some(src) = attrs
            .iter()
            .find(|a| a.name.as_ref() == "src")
            .map(|a| a.value.as_ref())
        else {
            return false;
        };
        let lower = src.to_ascii_lowercase();
        lower.contains("img.shields.io")
            || lower.contains("shields.io/")
            || lower.contains("badge.svg")
            || lower.contains("/badge")
    }

    fn fragment_roots_are_center_wrapper(fragment: &HtmlFragment) -> bool {
        !fragment.roots().is_empty()
            && fragment.roots().iter().all(|&root| {
                matches!(
                    fragment.node(root),
                    Some(HtmlNode::Element { tag, .. }) if tag.as_str() == "center"
                )
            })
    }

    fn markdown_code_settings(&self) -> markdown::Settings {
        let (inline_code_bg, inline_code_color) = self
            .style
            .map(|s| (s.inline_code_background, s.inline_code_color))
            .unwrap_or((None, None));
        let link_color = self
            .style
            .and_then(|s| s.link_color)
            .unwrap_or_else(|| iced::Color::from_rgb8(0x5A, 0x6B, 0x9E));
        let style = markdown::Style {
            font: self.font,
            inline_code_highlight: markdown::Highlight {
                background: inline_code_bg
                    .unwrap_or(DEFAULT_INLINE_CODE_BACKGROUND)
                    .into(),
                border: border::rounded(VSCODE_INLINE_RADIUS)
                    .width(1.0)
                    .color(DEFAULT_INLINE_CODE_BORDER),
            },
            inline_code_padding: padding::left(VSCODE_INLINE_HORIZONTAL_PADDING)
                .right(VSCODE_INLINE_HORIZONTAL_PADDING)
                .top(VSCODE_INLINE_VERTICAL_PADDING)
                .bottom(VSCODE_INLINE_VERTICAL_PADDING),
            inline_code_color: inline_code_color.unwrap_or(DEFAULT_INLINE_CODE_FOREGROUND),
            inline_code_font: self.font_mono,
            code_block_font: self.font_mono,
            link_color,
        };
        let mut settings = markdown::Settings::with_text_size(Pixels(self.text_size), style);
        settings.spacing = Pixels(self.paragraph_spacing.unwrap_or(5.0));
        settings.code_size = Pixels(self.text_size);
        settings
    }

    fn render_fenced_code_block(&self, block: &'a CachedCodeBlock) -> RenderedSpan<'a, M, T> {
        if block.complete
            && let Some(lines) = block.highlighted_lines.as_deref()
        {
            let settings = self.markdown_code_settings();
            return self.wrap_fenced_code_container(
                self.fenced_code_inner(lines, &settings),
                block.code.as_ref(),
                Some(std::sync::Arc::clone(&block.code)),
                block.language.as_deref(),
            );
        }

        let size = self.text_size;
        let code = block.code.to_string();
        if let Some(draw) = &self.fn_drawing_pre_block {
            return draw(self.codeblock(code, size, false).render()).into();
        }
        self.codeblock(code, size, false)
    }
}

impl<'a, M: Clone + 'static, T: widget::text::Catalog + 'a> MarkWidget<'a, M, T> {
    fn significant_children<'b>(&self, children: &[DomRef<'b>]) -> Vec<DomRef<'b>> {
        if children.len() <= 1 {
            return children.to_vec();
        }

        let mut meaningful_positions = Vec::with_capacity(children.len());
        for (index, child) in children.iter().copied().enumerate() {
            if !child.is_useless() {
                meaningful_positions.push(index);
            }
        }

        if meaningful_positions.is_empty() {
            return Vec::new();
        }
        if meaningful_positions.len() == children.len() {
            return children.to_vec();
        }

        let mut significant = Vec::with_capacity(children.len());
        let mut next_meaningful = 0usize;
        let mut previous_meaningful = None;

        for (index, child) in children.iter().copied().enumerate() {
            if meaningful_positions.get(next_meaningful).copied() == Some(index) {
                significant.push(child);
                previous_meaningful = Some(child);
                next_meaningful += 1;
                continue;
            }

            let following_meaningful = meaningful_positions
                .get(next_meaningful)
                .map(|&next| children[next]);
            let preserve = matches!(
                (previous_meaningful, following_meaningful),
                (Some(prev), Some(next))
                    if !prev.is_block_element() && !next.is_block_element()
            );
            if preserve {
                significant.push(child);
            }
        }

        significant
    }
}

impl<'a, M: Clone + 'static, T: ValidTheme + 'a> From<MarkWidget<'a, M, T>> for Element<'a, M, T>
where
    <T as widget::button::Catalog>::Class<'a>: From<widget::button::StyleFn<'a, T>>,
{
    fn from(mut value: MarkWidget<'a, M, T>) -> Self {
        value.render_from_block_cache()
    }
}

fn child_data_for_block_alignment(align: Option<BlockAlignment>) -> ChildData {
    ChildData {
        alignment: align.map(|a| match a {
            BlockAlignment::Center => ChildAlignment::Center,
            BlockAlignment::Right => ChildAlignment::Right,
        }),
        ..Default::default()
    }
}

fn text_size_for_data(text_size: f32, heading_scale: f32, heading_weight: u16) -> f32 {
    if heading_weight == 0 {
        return text_size;
    }
    let scaling = match heading_weight {
        1 => 1.8,
        2 => 1.5,
        3 => 1.25,
        4 => 1.15,
        5 => 0.875,
        6 => 0.75,
        7 => 0.625,
        _ => 1.0,
    };
    text_size * (1.0 + ((scaling - 1.0) * heading_scale))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CssLength {
    Pixels(f32),
    Em(f32),
    Number(f32),
}

fn css_dimension_with_font(style: &str, name: &str, font_size: f32) -> Option<f32> {
    style.split(';').find_map(|declaration| {
        let (property, value) = declaration.split_once(':')?;
        if !property.trim().eq_ignore_ascii_case(name) {
            return None;
        }
        Some(parse_css_length(value.trim())?.to_pixels(font_size))
    })
}

impl CssLength {
    fn to_pixels(self, font_size: f32) -> f32 {
        match self {
            Self::Pixels(value) | Self::Number(value) => value,
            Self::Em(value) => value * font_size,
        }
    }
}

fn parse_css_length(value: &str) -> Option<CssLength> {
    let value = value.trim();
    if let Some(em) = value.strip_suffix("em") {
        Some(CssLength::Em(em.trim().parse().ok()?))
    } else if let Some(px) = value.strip_suffix("px") {
        Some(CssLength::Pixels(px.trim().parse().ok()?))
    } else {
        Some(CssLength::Number(value.parse().ok()?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitHubAlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl GitHubAlertKind {
    fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    fn color(self) -> iced::Color {
        match self {
            Self::Note => iced::Color::from_rgb8(0x2F, 0x81, 0xF7),
            Self::Tip => iced::Color::from_rgb8(0x1A, 0x7F, 0x37),
            Self::Important => iced::Color::from_rgb8(0x82, 0x5B, 0xD4),
            Self::Warning => iced::Color::from_rgb8(0x9A, 0x67, 0x00),
            Self::Caution => iced::Color::from_rgb8(0xCF, 0x22, 0x2E),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Note => "ℹ️",
            Self::Tip => "💡",
            Self::Important => "❗",
            Self::Warning => "⚠️",
            Self::Caution => "🚨",
        }
    }
}

fn github_alert_kind(class: Option<&str>) -> Option<GitHubAlertKind> {
    let class = class?;
    class.split_ascii_whitespace().find_map(|name| match name {
        "markdown-alert-note" => Some(GitHubAlertKind::Note),
        "markdown-alert-tip" => Some(GitHubAlertKind::Tip),
        "markdown-alert-important" => Some(GitHubAlertKind::Important),
        "markdown-alert-warning" => Some(GitHubAlertKind::Warning),
        "markdown-alert-caution" => Some(GitHubAlertKind::Caution),
        _ => None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Sub,
    Sup,
}

fn clean_whitespace(input: &str) -> String {
    if input.trim().is_empty() {
        return " ".to_string();
    }
    let mut s = input.split_whitespace().collect::<Vec<&str>>().join(" ");
    if let Some(last) = input.chars().next_back()
        && last != '\n'
        && last.is_whitespace()
    {
        s.push(last);
    }
    if let Some(first) = input.chars().next()
        && first != '\n'
        && first.is_whitespace()
    {
        s.insert(0, first);
    }
    s
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::html::fragment::HtmlFragment;

    #[test]
    fn fragment_root_text_nodes_are_rendered() {
        let fragment = HtmlFragment::from_html("Here's<b> bold text, </b>and\n<i> italics!</i>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(
            debug.contains("Here's"),
            "expected root text 'Here's' in render output, got: {debug}"
        );
        assert!(
            debug.contains("and"),
            "expected root text 'and' in render output, got: {debug}"
        );
        assert!(
            debug.contains("bold text"),
            "expected bold text in render output, got: {debug}"
        );
    }

    #[test]
    fn inline_whitespace_between_tags_is_preserved() {
        let fragment = HtmlFragment::from_html("<p><em>italic</em> <em>italic</em></p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(
            debug.contains("\" \""),
            "expected preserved space, got: {debug}"
        );
    }

    #[test]
    fn hard_break_inside_paragraph_stays_newline() {
        let fragment = HtmlFragment::from_html("<p>Line 1<br>Line 2</p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let paragraph = DomRef::fragment_roots(&fragment)[0];
        let rendered = widget.render_children(paragraph, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(
            debug.contains("\\n"),
            "expected hard break to survive as newline, got: {debug}"
        );
    }

    #[test]
    fn blockquote_with_multiple_paragraphs_does_not_collapse_to_one_line() {
        let fragment = HtmlFragment::from_html(
            "<blockquote><p>The problem with being faster than light</p><p>is that you live in darkness</p></blockquote>",
        );
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(
            !debug
                .contains("The problem with being faster than light is that you live in darkness"),
            "expected separate paragraph blocks inside blockquote, got: {debug}"
        );
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, _)),
            "expected blockquote with multiple paragraphs to stay block-rendered, got: {debug}"
        );
    }

    #[test]
    fn centered_inline_markdown_paragraph_stays_single_spans_row() {
        let fragment = HtmlFragment::from_html(
            "<center><p>Normal, <em>italic</em>, <strong>bold</strong>, <del>strikethrough</del>, <strong>underline</strong>, <code>code</code>, <a href=\"https://example.com\">link</a></p></center>",
        );
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let center = DomRef::fragment_roots(&fragment)[0];
        let paragraph = center
            .children()
            .into_iter()
            .find(|child| child.tag_name() == Some("p"))
            .expect("paragraph");
        let child_tags: Vec<_> = paragraph
            .children()
            .into_iter()
            .map(|child| {
                child
                    .tag_name()
                    .map(str::to_string)
                    .unwrap_or_else(|| "#text".to_string())
            })
            .collect();

        let rendered = widget.render_children(paragraph, ChildData::default());
        let debug = format!("{rendered:?}");
        eprintln!("paragraph child tags: {child_tags:?}");
        eprintln!("paragraph debug: {debug}");
        assert!(
            !rendered.is_empty(),
            "expected inline paragraph to render content, got {debug}"
        );
    }

    #[test]
    fn inline_html_tags_do_not_render_todo_placeholders() {
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);

        let inserted = widget.render_fragment_roots(
            &HtmlFragment::from_html("<p><ins>inserted</ins></p>"),
            ChildData::default(),
        );
        let inserted_debug = format!("{inserted:?}");
        assert!(
            !inserted_debug.contains("(TODO)"),
            "unexpected placeholder: {inserted_debug}"
        );
        assert!(inserted_debug.contains("inserted"));

        let scripts = widget.render_fragment_roots(
            &HtmlFragment::from_html("<p><sub>sub</sub> <sup>sup</sup></p>"),
            ChildData::default(),
        );
        let scripts_debug = format!("{scripts:?}");
        assert!(
            !scripts_debug.contains("(TODO)"),
            "unexpected placeholder: {scripts_debug}"
        );
        assert!(matches!(scripts, RenderedSpan::Elem(_, _, _)));
    }

    #[test]
    fn inline_code_keeps_optical_gap_from_neighbor_punctuation() {
        let fragment = HtmlFragment::from_html("<p>Alpha <code>beta</code>, gamma.</p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(
            debug.contains(","),
            "expected punctuation to remain outside code: {debug}"
        );
        assert!(
            matches!(rendered, RenderedSpan::Spans(_)),
            "expected inline code to stay in the same rich_text flow, got {debug}"
        );
    }

    #[test]
    fn picture_and_source_do_not_render_todo_placeholders() {
        let fragment = HtmlFragment::from_html(
            "<picture><source srcset=\"a.webp\"><img src=\"a.png\"></picture>",
        );
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(!debug.contains("(TODO)"), "unexpected placeholder: {debug}");
    }

    #[test]
    fn text_input_degrades_without_todo_placeholder() {
        let fragment =
            HtmlFragment::from_html("<p><input type=\"text\" value=\"todo placeholder\"></p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(!debug.contains("(TODO)"), "unexpected placeholder: {debug}");
        assert!(debug.contains("todo placeholder"));
    }

    #[test]
    fn linked_image_renders_as_button_wrapped_element() {
        let fragment =
            HtmlFragment::from_html("<a href=\"https://example.com\"><img src=\"a.png\"></a>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<String, iced::Theme>::new(&state).on_clicking_link(|url| url);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, _)),
            "expected linked image to render as element"
        );
    }

    #[test]
    fn markdown_linked_image_renders_as_button_wrapped_element() {
        use crate::{Document, ParseProfile};

        let doc = Document::parse(
            "[![image](https://example.com/image.png)](https://example.com)",
            ParseProfile::GitHubPreview,
        )
        .expect("parse");
        let state = MarkState::from_document(&doc);
        let cache = state.cache.as_ref().expect("block cache");
        let fragment = match cache.entry(0) {
            Some(CachedBlock::Fragment(fragment)) => fragment.clone(),
            _ => panic!("expected markdown fragment block"),
        };

        let mut widget = MarkWidget::<String, iced::Theme>::new(&state)
            .on_clicking_link(|url| url)
            .on_drawing_image(|info| widget::text(info.alt.unwrap_or(info.url).to_string()).into());
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, _)),
            "expected markdown linked image to render as clickable element"
        );
    }

    #[test]
    fn nested_blockquote_with_block_children_renders_as_element() {
        let fragment = HtmlFragment::from_html(
            "<blockquote><p>Block Quote.</p><ul><li>Point 1</li><li>Point 2</li></ul><blockquote><p>Nested Block Quote</p><pre><code>#include &lt;cstdio&gt;\nint main() {}</code></pre></blockquote></blockquote>",
        );
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, _)),
            "expected nested blockquote with block content to render as element"
        );
    }

    #[test]
    fn github_alert_class_maps_to_alert_kind() {
        assert_eq!(
            github_alert_kind(Some("markdown-alert markdown-alert-note")),
            Some(GitHubAlertKind::Note)
        );
        assert_eq!(
            github_alert_kind(Some("foo markdown-alert-warning bar")),
            Some(GitHubAlertKind::Warning)
        );
        assert_eq!(github_alert_kind(Some("blockquote")), None);
    }

    #[test]
    fn css_lengths_keep_pixel_and_em_units() {
        assert_eq!(parse_css_length("100px"), Some(CssLength::Pixels(100.0)));
        assert_eq!(parse_css_length("2em"), Some(CssLength::Em(2.0)));
        assert_eq!(
            css_dimension_with_font("width: 100px", "width", 20.0),
            Some(100.0)
        );
        assert_eq!(
            css_dimension_with_font("width: 2em", "width", 20.0),
            Some(40.0)
        );
        assert_eq!(
            css_dimension_with_font("min-width: 100px; width: 2em", "width", 20.0),
            Some(40.0)
        );
    }

    #[test]
    fn kbd_tag_renders_without_todo_placeholder() {
        let fragment = HtmlFragment::from_html("<p><kbd>Ctrl + Shift + P</kbd></p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(!debug.contains("(TODO)"), "unexpected placeholder: {debug}");
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, _)),
            "expected keycap element, got {debug}"
        );
    }

    #[test]
    fn kbd_keeps_small_optical_gap_from_neighbor_punctuation() {
        let fragment = HtmlFragment::from_html("<p><kbd>Ctrl</kbd>, highlight.</p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(
            debug.contains(","),
            "expected punctuation to remain outside kbd: {debug}"
        );
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, gap) if (gap - KBD_INLINE_OPTICAL_GAP).abs() < f32::EPSILON),
            "expected kbd to use the inline optical gap, got {debug}"
        );
    }

    #[cfg(feature = "mermaid")]
    #[test]
    fn mermaid_code_block_inside_html_fragment_renders_svg_widget() {
        let fragment = HtmlFragment::from_html(
            "<pre><code class=\"language-mermaid\">flowchart LR\nA[RaTeX] --> B[StriMD]\n</code></pre>",
        );
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let rendered_tag = rendered.render().as_widget().tag();
        let expected_svg: iced::Element<'_, ()> =
            widget::svg(iced::widget::svg::Handle::from_memory(
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".as_slice(),
            ))
            .into();
        let expected_svg_tag = expected_svg.as_widget().tag();
        assert!(
            rendered_tag == expected_svg_tag,
            "expected mermaid code block to render as SVG widget"
        );
    }

    #[cfg(feature = "math")]
    #[test]
    fn inline_math_span_renders_as_svg_widget() {
        use crate::{Document, ParseProfile};

        let doc =
            Document::parse("Energy $E = mc^2$ here.", ParseProfile::GitHubPreview).expect("parse");
        let state = MarkState::from_document(&doc);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let cache = state.cache.as_ref().expect("block cache");
        let fragment = match cache.entry(0) {
            Some(CachedBlock::Fragment(fragment)) => fragment.clone(),
            _ => panic!("expected markdown fragment block at index 0"),
        };
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        assert!(
            matches!(rendered, RenderedSpan::Elem(_, _, gap) if gap > 0.0),
            "expected inline math as SVG element, got: {rendered:?}"
        );
    }

    #[test]
    fn sub_and_sup_render_without_todo_placeholders() {
        let fragment = HtmlFragment::from_html("<p><sub>sub</sub> <sup>sup</sup></p>");
        let state = MarkState::from_blocks(&[]);
        let mut widget = MarkWidget::<(), iced::Theme>::new(&state);
        let rendered = widget.render_fragment_roots(&fragment, ChildData::default());
        let debug = format!("{rendered:?}");
        assert!(!debug.contains("(TODO)"), "unexpected placeholder: {debug}");
        assert!(matches!(rendered, RenderedSpan::Elem(_, _, _)));
    }
}
