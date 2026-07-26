use std::sync::Arc;

use crate::core::block::BlockContent;
use crate::core::error::UnsupportedReason;
use crate::html::fragment::{HtmlAttr, HtmlFragment, HtmlNode, HtmlTag, NodeId};
use crate::options::RawHtmlPolicy;

/// Tags that must never be rendered or exported as live HTML.
const UNSAFE_TAGS: &[&str] = &[
    "base", "embed", "iframe", "link", "math", "meta", "object", "script", "style", "svg",
    "template",
];

/// Attributes that can execute script or navigate a resource.
const URL_ATTRIBUTES: &[&str] = &[
    "action",
    "archive",
    "background",
    "cite",
    "classid",
    "code",
    "data",
    "formaction",
    "href",
    "longdesc",
    "manifest",
    "poster",
    "ping",
    "profile",
    "src",
    "srcset",
    "usemap",
    "xlink:href",
];

#[cfg(feature = "static")]
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Build block content from raw HTML according to [`RawHtmlPolicy`].
#[must_use]
pub fn block_content_from_raw_html(
    html: &str,
    policy: RawHtmlPolicy,
    gfm_tagfilter: bool,
) -> BlockContent {
    match policy {
        RawHtmlPolicy::Preserve => {
            let html = if gfm_tagfilter {
                crate::html::tagfilter::apply_gfm_tagfilter(html)
            } else {
                html.to_string()
            };
            BlockContent::Html(HtmlFragment::from_html(&html))
        }
        RawHtmlPolicy::Escape => BlockContent::Html(text_fragment(html)),
        RawHtmlPolicy::StripUnsupported => {
            let html = if gfm_tagfilter {
                crate::html::tagfilter::apply_gfm_tagfilter(html)
            } else {
                html.to_string()
            };
            if let Some(tag) = unsafe_tag_in_raw_html(&html) {
                return BlockContent::Unsupported {
                    reason: UnsupportedReason::HtmlTag(tag),
                };
            }
            let sanitized_html = sanitize_inline_html_source(&html).unwrap_or_else(|| html.clone());
            content_from_fragment(HtmlFragment::from_html(&sanitized_html))
        }
    }
}

fn unsafe_tag_in_raw_html(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    for tag in UNSAFE_TAGS {
        for needle in [format!("<{tag}"), format!("</{tag}")] {
            let mut search_from = 0;
            while let Some(relative_pos) = lower[search_from..].find(&needle) {
                let pos = search_from + relative_pos;
                let after = lower.as_bytes().get(pos + needle.len());
                if matches!(
                    after,
                    Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
                ) {
                    return Some(tag.to_string());
                }
                search_from = pos + needle.len();
            }
        }
    }
    None
}

/// Reject or keep a parsed [`HtmlFragment`] based on unsafe tag policy.
#[must_use]
pub fn content_from_fragment(fragment: HtmlFragment) -> BlockContent {
    match sanitize_fragment(fragment) {
        Ok(fragment) => BlockContent::Html(fragment),
        Err(tag) => BlockContent::Unsupported {
            reason: UnsupportedReason::HtmlTag(tag),
        },
    }
}

/// Convert an inline raw-HTML event into a safe source string for
/// `RawHtmlPolicy::StripUnsupported`. `None` means the event must be emitted as
/// text (which causes the Markdown writer to escape it).
pub(crate) fn sanitize_inline_html_source(html: &str) -> Option<String> {
    if unsafe_tag_in_raw_html(html).is_some() {
        return None;
    }

    // Keep the original event shape (especially a standalone opening tag)
    // while removing dangerous attributes. Parsing and serializing an inline
    // event would manufacture a closing tag for `<a ...>`, breaking the
    // matching `</a>` event that follows it.
    Some(strip_dangerous_attributes_lexically(html))
}

fn strip_dangerous_attributes_lexically(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(relative_start) = html[cursor..].find('<') {
        let start = cursor + relative_start;
        out.push_str(&html[cursor..start]);
        let Some(relative_end) = html_tag_end(&html[start..]) else {
            out.push_str(&html[start..]);
            break;
        };
        let end = start + relative_end + 1;
        out.push_str(&sanitize_tag_lexically(&html[start..end]));
        cursor = end;
    }
    if cursor < html.len() {
        out.push_str(&html[cursor..]);
    }
    out
}

fn sanitize_tag_lexically(tag: &str) -> String {
    if tag.starts_with("<!--")
        || tag.starts_with("</")
        || tag.starts_with("<!")
        || tag.starts_with("<?")
    {
        return tag.to_string();
    }
    let bytes = tag.as_bytes();
    if bytes.len() < 3 {
        return tag.to_string();
    }
    let mut name_end = 1;
    while name_end < bytes.len() - 1
        && !bytes[name_end].is_ascii_whitespace()
        && !matches!(bytes[name_end], b'/' | b'>')
    {
        name_end += 1;
    }
    let mut out = String::from(&tag[..name_end]);
    let mut i = name_end;
    while i < bytes.len() - 1 {
        let attr_start = i;
        while i < bytes.len() - 1 && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() - 1 {
            out.push_str(&tag[attr_start..bytes.len() - 1]);
            break;
        }
        if bytes[i] == b'/' {
            out.push_str(&tag[attr_start..bytes.len() - 1]);
            break;
        }
        let name_start = i;
        while i < bytes.len() - 1
            && !bytes[i].is_ascii_whitespace()
            && !matches!(bytes[i], b'=' | b'/' | b'>')
        {
            i += 1;
        }
        if name_start == i {
            out.push_str(&tag[attr_start..=i]);
            i += 1;
            continue;
        }
        let name = &tag[name_start..i];
        while i < bytes.len() - 1 && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = "";
        if i < bytes.len() - 1 && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() - 1 && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let value_start = i;
            if i < bytes.len() - 1 && matches!(bytes[i], b'\'' | b'\"') {
                let quote = bytes[i];
                i += 1;
                let content_start = i;
                while i < bytes.len() - 1 && bytes[i] != quote {
                    i += 1;
                }
                value = &tag[content_start..i.min(bytes.len() - 1)];
                if i < bytes.len() - 1 {
                    i += 1;
                }
            } else {
                while i < bytes.len() - 1 && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = &tag[value_start..i];
            }
        }
        let attr = HtmlAttr {
            name: Arc::from(name.to_ascii_lowercase()),
            value: Arc::from(value),
        };
        if !dangerous_attribute(&attr) {
            out.push_str(&tag[attr_start..i]);
        }
    }
    out.push('>');
    out
}

fn html_tag_end(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut quote = None;
    for (index, byte) in bytes.iter().copied().enumerate().skip(1) {
        match quote {
            Some(active) if byte == active => quote = None,
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => return Some(index),
            _ => {}
        }
    }
    None
}

fn text_fragment(text: &str) -> HtmlFragment {
    let mut fragment = HtmlFragment::empty();
    let id = fragment.push_text(Arc::from(text));
    fragment.push_root(id);
    fragment
}

fn sanitize_fragment(fragment: HtmlFragment) -> Result<HtmlFragment, String> {
    sanitize_fragment_with_changes(fragment).map(|(fragment, _)| fragment)
}

fn sanitize_fragment_with_changes(fragment: HtmlFragment) -> Result<(HtmlFragment, bool), String> {
    let mut sanitized = HtmlFragment::empty();
    let mut changed = false;
    for &root in fragment.roots() {
        let (id, node_changed) = clone_sanitized_node(&fragment, root, &mut sanitized)?;
        changed |= node_changed;
        sanitized.push_root(id);
    }
    Ok((sanitized, changed))
}

fn clone_sanitized_node(
    source: &HtmlFragment,
    id: NodeId,
    target: &mut HtmlFragment,
) -> Result<(NodeId, bool), String> {
    let Some(node) = source.node(id) else {
        return Err("missing HTML fragment node".into());
    };
    match node {
        HtmlNode::Text(text) => Ok((target.push_text(text.clone()), false)),
        HtmlNode::Comment(comment) => Ok((target.push_comment(comment.clone()), false)),
        HtmlNode::Element {
            tag,
            attrs,
            children,
        } => {
            if UNSAFE_TAGS.contains(&tag.as_str()) {
                return Err(tag.as_str().to_string());
            }
            let mut changed = false;
            let safe_attrs: Vec<HtmlAttr> = attrs
                .iter()
                .filter_map(|attr| {
                    if dangerous_attribute(attr) {
                        changed = true;
                        None
                    } else {
                        Some(attr.clone())
                    }
                })
                .collect();
            let mut safe_children = Vec::with_capacity(children.len());
            for child in children {
                let (child_id, child_changed) = clone_sanitized_node(source, *child, target)?;
                changed |= child_changed;
                safe_children.push(child_id);
            }
            Ok((
                target.push_element(HtmlTag::new(tag.0.clone()), safe_attrs, safe_children),
                changed,
            ))
        }
    }
}

fn dangerous_attribute(attr: &HtmlAttr) -> bool {
    let name = attr.name.to_ascii_lowercase();
    if name.starts_with("on") || matches!(name.as_str(), "srcdoc" | "style") {
        return true;
    }
    URL_ATTRIBUTES.contains(&name.as_str()) && dangerous_url(&attr.value)
}

fn dangerous_url(value: &str) -> bool {
    let normalized: String = value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && !ch.is_ascii_control())
        .flat_map(char::to_lowercase)
        .collect();
    if normalized.contains("&#") || normalized.contains("&colon;") {
        // Character references are decoded by html5ever but may still be
        // present in the no-static lexical fallback. Treat them as unsafe
        // rather than allowing an encoded protocol prefix through.
        return true;
    }
    normalized.starts_with("javascript:")
        || normalized.starts_with("vbscript:")
        || normalized.starts_with("data:")
        || normalized.starts_with("file:")
}

/// Serialize a parsed fragment with escaped text and HTML void-element rules.
/// This is shared by the static writer and inline-policy sanitizer.
#[cfg(feature = "static")]
pub(crate) fn serialize_fragment(fragment: &HtmlFragment) -> Result<String, String> {
    let mut out = String::new();
    for &root in fragment.roots() {
        serialize_node(&mut out, fragment, root, false)?;
    }
    Ok(out)
}

#[cfg(feature = "static")]
fn serialize_node(
    out: &mut String,
    fragment: &HtmlFragment,
    id: NodeId,
    raw_text_parent: bool,
) -> Result<(), String> {
    let Some(node) = fragment.node(id) else {
        return Err(format!("missing HtmlFragment node {id:?}"));
    };
    match node {
        HtmlNode::Text(text) if raw_text_parent => out.push_str(text),
        HtmlNode::Text(text) => out.push_str(&html_escape_text_node(text)),
        HtmlNode::Comment(comment) => {
            out.push_str("<!--");
            out.push_str(comment);
            out.push_str("-->");
        }
        HtmlNode::Element {
            tag,
            attrs,
            children,
        } => {
            out.push('<');
            out.push_str(tag.as_str());
            for attr in attrs {
                out.push(' ');
                out.push_str(&attr.name);
                out.push_str("=\"");
                out.push_str(&html_escape_text(&attr.value));
                out.push('\"');
            }
            out.push('>');
            let raw_text_child = matches!(tag.as_str(), "script" | "style");
            for child in children {
                serialize_node(out, fragment, *child, raw_text_child)?;
            }
            if !VOID_ELEMENTS.contains(&tag.as_str()) {
                out.push_str("</");
                out.push_str(tag.as_str());
                out.push('>');
            }
        }
    }
    Ok(())
}

#[cfg(feature = "static")]
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

#[cfg(feature = "static")]
fn html_escape_text_node(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::fragment::HtmlNode;

    #[test]
    fn preserve_keeps_details() {
        let content = block_content_from_raw_html(
            "<details><summary>x</summary></details>",
            RawHtmlPolicy::Preserve,
            false,
        );
        assert!(matches!(content, BlockContent::Html(_)));
    }

    #[test]
    fn strip_unsupported_rejects_script() {
        let content = block_content_from_raw_html(
            "<p>ok</p><script>alert(1)</script>",
            RawHtmlPolicy::StripUnsupported,
            false,
        );
        assert!(matches!(
            content,
            BlockContent::Unsupported {
                reason: UnsupportedReason::HtmlTag(_)
            }
        ));
    }

    #[test]
    fn escape_policy_preserves_source_as_text() {
        let content = block_content_from_raw_html("<span>x</span>", RawHtmlPolicy::Escape, false);
        let BlockContent::Html(fragment) = content else {
            panic!("expected escaped source as text fragment");
        };
        assert!(matches!(
            fragment.node(fragment.roots()[0]),
            Some(HtmlNode::Text(text)) if text.as_ref() == "<span>x</span>"
        ));
    }

    #[cfg(feature = "static")]
    #[test]
    fn strip_unsupported_removes_event_handlers_and_javascript_urls() {
        let content = block_content_from_raw_html(
            "<a href=\"javascript:alert(1)\" onclick=\"alert(2)\">x</a>",
            RawHtmlPolicy::StripUnsupported,
            false,
        );
        let BlockContent::Html(fragment) = content else {
            panic!("expected sanitized fragment");
        };
        let Some(HtmlNode::Element { attrs, .. }) = fragment.node(fragment.roots()[0]) else {
            panic!("expected anchor element");
        };
        assert!(attrs.is_empty(), "dangerous attrs survived: {attrs:?}");
    }

    #[test]
    fn nested_script_is_detected() {
        let content = block_content_from_raw_html(
            "<div><script>x</script></div>",
            RawHtmlPolicy::StripUnsupported,
            false,
        );
        assert!(matches!(content, BlockContent::Unsupported { .. }));
    }

    #[test]
    fn unsafe_tag_detection_checks_past_similar_prefixes() {
        assert_eq!(
            unsafe_tag_in_raw_html("<scriptx>ok</scriptx><script>alert(1)</script>"),
            Some("script".into())
        );
    }

    #[cfg(not(feature = "static"))]
    #[test]
    fn lexical_strip_rejects_encoded_url_protocols() {
        let sanitized = sanitize_inline_html_source(
            "<a href=\"java&#x73;cript:alert(1)\" onclick=\"alert(2)\">x</a>",
        )
        .expect("safe text fallback");
        assert!(!sanitized.contains("href="));
        assert!(!sanitized.contains("onclick="));
    }

    #[cfg(feature = "static")]
    #[test]
    fn gfm_tagfilter_escapes_disallowed_raw_html() {
        let content = block_content_from_raw_html("<xmp>bad</xmp>", RawHtmlPolicy::Preserve, true);
        let BlockContent::Html(fragment) = content else {
            panic!("expected html fragment");
        };
        assert_eq!(fragment.roots().len(), 1);
        assert!(matches!(
            fragment.node(fragment.roots()[0]),
            Some(HtmlNode::Text(text)) if text.contains("<xmp>")
        ));
    }

    #[cfg(not(feature = "static"))]
    #[test]
    fn gfm_tagfilter_keeps_escaped_source_without_html_parser() {
        let content = block_content_from_raw_html("<xmp>bad</xmp>", RawHtmlPolicy::Preserve, true);
        let BlockContent::Html(fragment) = content else {
            panic!("expected html fragment");
        };
        assert!(matches!(
            fragment.node(fragment.roots()[0]),
            Some(HtmlNode::Text(text)) if text.contains("&lt;xmp>")
        ));
    }
}
