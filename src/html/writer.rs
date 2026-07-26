use pulldown_cmark::html;

use crate::core::block::{BlockContent, RenderBlock};
use crate::core::error::RenderError;

/// Write a slice of blocks to an HTML string using pulldown's HTML writer.
pub fn blocks_to_html(blocks: &[RenderBlock]) -> Result<String, RenderError> {
    let mut out = String::new();
    for block in blocks {
        match &block.content {
            BlockContent::Markdown(compiled) => {
                html::push_html(&mut out, compiled.events().iter().cloned());
            }
            BlockContent::Html(fragment) => {
                let serialized = crate::html::sanitize::serialize_fragment(fragment)
                    .map_err(RenderError::new)?;
                out.push_str(&serialized);
            }
            BlockContent::Code { lang, complete: _ } => {
                let lang_attr = lang
                    .as_deref()
                    .and_then(|l| l.split_whitespace().next())
                    .map(|l| format!(" class=\"language-{}\"", html_escape_text(l)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "<pre><code{lang_attr}>{}</code></pre>",
                    html_escape_text(&block.source)
                ));
            }
            BlockContent::PendingMarkdown => {}
            #[cfg(feature = "math")]
            BlockContent::Math { latex, display } => {
                let (open, close) = if *display { ("$$", "$$") } else { ("$", "$") };
                out.push_str(&format!(
                    "<span class=\"math math-display\">{open}{}{close}</span>",
                    html_escape_text(latex)
                ));
            }
            #[cfg(feature = "mermaid")]
            BlockContent::Mermaid { source, .. } => {
                out.push_str(&format!(
                    "<pre><code class=\"language-mermaid\">{}</code></pre>",
                    html_escape_text(source)
                ));
            }
            BlockContent::Unsupported { reason } => {
                out.push_str(&format!(
                    "<!-- unsupported: {} -->",
                    html_escape_text(&format!("{reason:?}"))
                ));
            }
        }
    }
    Ok(out)
}

fn html_escape_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::core::block::{BlockKind, BlockStatus, RenderBlock};
    use crate::core::ids::BlockId;
    use crate::html::fragment::HtmlFragment;

    fn block(source: &str, content: BlockContent) -> RenderBlock {
        RenderBlock {
            id: BlockId::new(1),
            status: BlockStatus::Committed,
            kind: BlockKind::HtmlBlock,
            source: Arc::from(source),
            content,
        }
    }

    #[test]
    fn escapes_fragment_text_and_does_not_close_void_elements() {
        let fragment = HtmlFragment::from_html("<pre><code>&lt;b&gt;</code></pre><br>");
        let html = blocks_to_html(&[block("source", BlockContent::Html(fragment))]).expect("html");
        assert!(html.contains("&lt;b>"), "text was not escaped: {html}");
        assert!(html.contains("<br>"), "void element missing: {html}");
        assert!(!html.contains("</br>"), "void element was closed: {html}");
    }

    #[test]
    fn preserves_raw_text_element_contents() {
        let fragment = HtmlFragment::from_html(
            "<div><script>if (a < b) { console.log(\"& value\"); }</script><style>.x > .y { color: red; }</style></div>",
        );
        let html = blocks_to_html(&[block("source", BlockContent::Html(fragment))]).expect("html");
        assert!(
            html.contains("if (a < b) { console.log(\"& value\"); }"),
            "html: {html}"
        );
        assert!(html.contains(".x > .y { color: red; }"), "html: {html}");
    }

    #[test]
    fn code_language_is_tokenized_and_attribute_escaped() {
        let html = blocks_to_html(&[block(
            "source",
            BlockContent::Code {
                lang: Some("rust\" onerror=\"alert(1)\" extra".into()),
                complete: true,
            },
        )])
        .expect("html");
        assert!(
            html.contains("class=\"language-rust&quot;"),
            "language attr: {html}"
        );
        assert!(!html.contains("onerror=\""), "attribute injection: {html}");
        assert!(
            !html.contains(" extra"),
            "language info string was not tokenized: {html}"
        );
    }
}
