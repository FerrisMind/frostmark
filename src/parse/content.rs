use std::sync::Arc;

#[cfg(feature = "math")]
use pulldown_cmark::TagEnd;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Tag};

use crate::core::block::{BlockContent, CompiledMarkdown};
use crate::html::sanitize;
use crate::options::RawHtmlPolicy;

/// True when the slice is a single fenced or indented code block.
pub(crate) fn is_code_fence_slice(slice: &[Event<'static>]) -> bool {
    matches!(slice.first(), Some(Event::Start(Tag::CodeBlock(_))))
}

/// Concatenate `Text` events inside a code block slice.
pub(crate) fn code_text_from_events(slice: &[Event<'static>]) -> String {
    slice
        .iter()
        .filter_map(|event| match event {
            Event::Text(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// True when the fenced code block language is Mermaid.
#[cfg(feature = "mermaid")]
pub(crate) fn is_mermaid_lang(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("mermaid")
}

/// LaTeX body from a display-math event slice.
#[cfg(feature = "math")]
pub(crate) fn display_math_latex_from_events(slice: &[Event<'static>]) -> Option<Arc<str>> {
    match slice.first()? {
        Event::DisplayMath(text) => Some(Arc::from(text.as_ref())),
        _ => None,
    }
}

/// Strip `$$` / `$` wrappers from a math block source string.
#[cfg(feature = "math")]
pub(crate) fn strip_math_delimiters(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() >= 4 {
        return trimmed[2..trimmed.len() - 2].trim().to_string();
    }
    if trimmed.starts_with('$') && trimmed.ends_with('$') && trimmed.len() >= 2 {
        return trimmed[1..trimmed.len() - 1].trim().to_string();
    }
    trimmed.to_string()
}

/// Language tag from a fenced code block, if present.
pub(crate) fn code_lang_from_events(slice: &[Event<'static>]) -> Option<String> {
    let Event::Start(Tag::CodeBlock(kind)) = slice.first()? else {
        return None;
    };
    match kind {
        CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
        _ => None,
    }
}

/// Display-math body when a paragraph block is only `$$…$$` / [`Event::DisplayMath`].
#[cfg(feature = "math")]
pub(crate) fn display_math_from_paragraph_slice(slice: &[Event<'static>]) -> Option<Arc<str>> {
    let inner: Vec<&Event<'_>> = slice
        .iter()
        .filter(|event| {
            !matches!(
                event,
                Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph)
            )
        })
        .collect();
    match inner.as_slice() {
        [Event::DisplayMath(text)] => Some(Arc::from(text.as_ref())),
        [Event::Text(text)] => {
            let stripped = strip_math_delimiters(text);
            if stripped != text.as_ref() && !stripped.is_empty() {
                Some(Arc::from(stripped))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Derive block content from pulldown events, routing raw HTML per [`RawHtmlPolicy`].
pub fn block_content_from_events(
    slice: &[Event<'static>],
    source: Arc<str>,
    raw_html: RawHtmlPolicy,
    gfm_tagfilter: bool,
) -> BlockContent {
    #[cfg(feature = "math")]
    if matches!(slice.first(), Some(Event::Start(Tag::Paragraph)))
        && let Some(latex) = display_math_from_paragraph_slice(slice)
    {
        return BlockContent::Math {
            latex,
            display: true,
        };
    }

    if is_code_fence_slice(slice) {
        let lang = code_lang_from_events(slice);
        #[cfg(feature = "mermaid")]
        if lang.as_deref().is_some_and(is_mermaid_lang) {
            return BlockContent::Mermaid {
                source: Arc::from(code_text_from_events(slice)),
                complete: true,
            };
        }
        return BlockContent::Code {
            lang,
            complete: true,
        };
    }
    if is_standalone_html_block(slice)
        && let Some(html) = extract_html_from_events(slice)
    {
        return sanitize::block_content_from_raw_html(&html, raw_html, gfm_tagfilter);
    }
    let events = match raw_html {
        RawHtmlPolicy::Preserve => slice.to_vec(),
        RawHtmlPolicy::Escape => slice
            .iter()
            .map(|event| match event {
                Event::Html(text) | Event::InlineHtml(text) => Event::Text(text.clone()),
                other => other.clone(),
            })
            .collect(),
        RawHtmlPolicy::StripUnsupported => slice
            .iter()
            .map(|event| match event {
                Event::Html(text) => sanitize::sanitize_inline_html_source(text)
                    .map(|safe| Event::Html(CowStr::Boxed(safe.into_boxed_str())))
                    .unwrap_or_else(|| Event::Text(text.clone())),
                Event::InlineHtml(text) => sanitize::sanitize_inline_html_source(text)
                    .map(|safe| Event::InlineHtml(CowStr::Boxed(safe.into_boxed_str())))
                    .unwrap_or_else(|| Event::Text(text.clone())),
                other => other.clone(),
            })
            .collect(),
    };
    BlockContent::Markdown(CompiledMarkdown::new(source, events))
}

/// True when the slice is a raw HTML block, not Markdown with embedded inline HTML.
fn is_standalone_html_block(slice: &[Event<'static>]) -> bool {
    let Some(first) = slice.first() else {
        return false;
    };
    match first {
        Event::Start(Tag::HtmlBlock) | Event::Html(_) => !slice.iter().any(|event| {
            matches!(
                event,
                Event::Start(Tag::Paragraph)
                    | Event::Start(Tag::Heading { .. })
                    | Event::Start(Tag::List(_))
                    | Event::Start(Tag::BlockQuote(_))
                    | Event::Start(Tag::Table(_))
                    | Event::Start(Tag::CodeBlock(_))
                    | Event::Start(Tag::FootnoteDefinition(_))
                    | Event::Start(Tag::Item)
                    | Event::Start(Tag::DefinitionList)
                    | Event::Start(Tag::DefinitionListTitle)
                    | Event::Start(Tag::DefinitionListDefinition)
                    | Event::Start(Tag::MetadataBlock(_))
            )
        }),
        _ => false,
    }
}

pub(crate) fn extract_html_from_events(slice: &[Event<'static>]) -> Option<String> {
    let mut html = String::new();
    for event in slice {
        match event {
            Event::Html(text) | Event::InlineHtml(text) => html.push_str(text),
            Event::Text(text) if matches!(slice.first(), Some(Event::Start(Tag::HtmlBlock))) => {
                html.push_str(text);
            }
            _ => {}
        }
    }
    if html.is_empty() { None } else { Some(html) }
}

/// Detect whether events or block kind indicate raw HTML content.
#[cfg(feature = "stream")]
pub fn events_contain_html(events: &[Event<'static>]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::Html(_) | Event::InlineHtml(_) | Event::Start(Tag::HtmlBlock)
        )
    })
}

/// Build HTML fragment content from raw source when no compiled events exist.
#[cfg(feature = "stream")]
pub fn html_block_content(
    source: Arc<str>,
    raw_html: RawHtmlPolicy,
    gfm_tagfilter: bool,
) -> BlockContent {
    sanitize::block_content_from_raw_html(&source, raw_html, gfm_tagfilter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulldown_cmark::{Options, Parser};

    #[test]
    fn multiline_html_block_preserves_details_children() {
        let source = "<details>\n<summary>Summary</summary>\nBody\n</details>\n";
        let events: Vec<_> = Parser::new_ext(source, pulldown_cmark::Options::all())
            .map(|e| e.into_static())
            .collect();
        let extracted = super::extract_html_from_events(&events).expect("html bytes");
        assert!(extracted.contains("summary"), "extracted: {extracted:?}");
        let content = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::Preserve,
            true,
        );
        assert!(matches!(content, BlockContent::Html(_)));
    }

    #[cfg(feature = "static")]
    #[test]
    fn html_block_fixture_preserves_details_children() {
        let source = "<details><summary>Summary</summary>Body</details>";
        let events: Vec<_> = Parser::new_ext(source, pulldown_cmark::Options::all())
            .map(|e| e.into_static())
            .collect();
        let content = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::Preserve,
            true,
        );
        let BlockContent::Html(fragment) = content else {
            panic!("expected html fragment");
        };
        let html = {
            #[cfg(feature = "static")]
            {
                use crate::core::block::{BlockKind, BlockStatus, RenderBlock};
                use crate::core::ids::BlockId;
                use crate::html::writer;
                let block = RenderBlock {
                    id: BlockId::new(1),
                    status: BlockStatus::Committed,
                    kind: BlockKind::HtmlBlock,
                    source: Arc::from(source),
                    content: BlockContent::Html(fragment),
                };
                writer::blocks_to_html(&[block]).expect("html")
            }
            #[cfg(not(feature = "static"))]
            String::new()
        };
        assert!(html.contains("summary"), "html: {html}");
    }

    /// Regression: README/doctest sample must stay Markdown (not a lone `<b>` Html block).
    #[test]
    fn readme_doctest_sample_stays_markdown() {
        let text = "Hello from **markdown** and <b>HTML</b>!";
        let events: Vec<_> = Parser::new_ext(text, pulldown_cmark::Options::all())
            .map(|e| e.into_static())
            .collect();
        let slice = events.as_slice();
        let content = block_content_from_events(
            slice,
            Arc::from(text),
            crate::options::RawHtmlPolicy::Preserve,
            true,
        );
        let BlockContent::Markdown(compiled) = content else {
            panic!("expected markdown block for readme sample, got {content:?}");
        };
        let html = {
            let mut buf = String::new();
            pulldown_cmark::html::push_html(&mut buf, compiled.events().iter().cloned());
            buf
        };
        assert!(html.contains("Hello"));
        assert!(html.contains("markdown"));
        assert!(html.contains("HTML"));
    }

    #[test]
    fn inline_html_stays_in_markdown_block() {
        let source = "text <span>x</span> and more";
        let events: Vec<_> = Parser::new_ext(source, Options::empty())
            .map(|e| e.into_static())
            .collect();
        let content = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::Preserve,
            false,
        );
        assert!(matches!(content, BlockContent::Markdown(_)));
    }

    #[test]
    fn code_fence_routes_to_block_content_code() {
        let source = "```rust\nfn main() {}\n```\n";
        let events: Vec<_> = Parser::new_ext(source, Options::all())
            .map(|e| e.into_static())
            .collect();
        let content = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::Preserve,
            true,
        );
        assert!(matches!(
            content,
            BlockContent::Code {
                lang: Some(ref l),
                complete: true,
            } if l == "rust"
        ));
        assert_eq!(code_text_from_events(&events), "fn main() {}\n");
    }

    #[test]
    fn inline_html_routes_to_fragment_for_html_only_block() {
        let source = "<details><summary>x</summary></details>";
        let events: Vec<_> = Parser::new_ext(source, Options::all())
            .map(|e| e.into_static())
            .collect();
        let content = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::Preserve,
            true,
        );
        assert!(matches!(content, BlockContent::Html(_)));
    }

    #[test]
    fn escape_policy_converts_inline_html_events_to_text() {
        let source = "text <script>alert(1)</script>";
        let events: Vec<_> = Parser::new_ext(source, Options::empty())
            .map(|e| e.into_static())
            .collect();
        let BlockContent::Markdown(compiled) = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::Escape,
            false,
        ) else {
            panic!("expected markdown block");
        };
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, compiled.events().iter().cloned());
        assert!(
            html.contains("&lt;script&gt;"),
            "inline HTML was not escaped: {html}"
        );
        assert!(!html.contains("<script>"), "live script survived: {html}");
    }

    #[test]
    fn strip_policy_removes_dangerous_inline_attributes() {
        let source = "text <a href=\"javascript:alert(1)\" onclick=\"alert(2)\">x</a>";
        let events: Vec<_> = Parser::new_ext(source, Options::empty())
            .map(|e| e.into_static())
            .collect();
        let BlockContent::Markdown(compiled) = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::StripUnsupported,
            false,
        ) else {
            panic!("expected markdown block");
        };
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, compiled.events().iter().cloned());
        assert!(
            !html.contains("javascript:"),
            "javascript URL survived: {html}"
        );
        assert!(!html.contains("onclick="), "event handler survived: {html}");
    }

    #[cfg(feature = "static")]
    #[test]
    fn strip_policy_preserves_inline_tag_shape() {
        let source = "text <a href=\"javascript:alert(1)\">x</a>";
        let events: Vec<_> = Parser::new_ext(source, Options::empty())
            .map(|e| e.into_static())
            .collect();
        let BlockContent::Markdown(compiled) = block_content_from_events(
            &events,
            Arc::from(source),
            crate::options::RawHtmlPolicy::StripUnsupported,
            false,
        ) else {
            panic!("expected markdown block");
        };
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, compiled.events().iter().cloned());
        assert_eq!(html, "<p>text <a>x</a></p>\n");
    }
}
