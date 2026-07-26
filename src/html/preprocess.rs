//! Optional deterministic HTML rewrite layer (`_html_preprocess` / `lol_html`).

use std::borrow::Cow;

/// Apply StriMD's fixed rewrite rules before html5ever parsing.
///
/// When `_html_preprocess` is disabled this is a no-op passthrough except for
/// legacy alignment normalization (`<div align="center">` → `<center>`).
#[must_use]
pub(crate) fn preprocess_raw_html(html: &str) -> Cow<'_, str> {
    let html = normalize_legacy_alignment_wrappers(html);
    #[cfg(feature = "_html_preprocess")]
    {
        match apply_lol_html_rewrites(html.as_ref()) {
            Ok(out) => Cow::Owned(out),
            Err(_) => html,
        }
    }
    #[cfg(not(feature = "_html_preprocess"))]
    {
        html
    }
}

/// html5ever drops obsolete `align` on `<div>`; map to `<center>` so iced alignment matches frostmark.
#[must_use]
pub(crate) fn normalize_legacy_alignment_wrappers(html: &str) -> Cow<'_, str> {
    let mut out = String::with_capacity(html.len());
    let mut div_stack: Vec<bool> = Vec::new();
    let mut i = 0;
    let bytes = html.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'<'
            && let Some(tag_end) = html_tag_end(&html[i..])
        {
            let tag = &html[i..=i + tag_end];
            if is_div_open_tag(tag) {
                let is_center = is_align_center_div_open(tag);
                div_stack.push(is_center);
                if is_center {
                    out.push_str("<center>");
                } else {
                    out.push_str(tag);
                }
                i = i + tag_end + 1;
                continue;
            }
            if is_div_close_tag(tag) {
                if div_stack.pop().unwrap_or(false) {
                    out.push_str("</center>");
                } else {
                    out.push_str(tag);
                }
                i = i + tag_end + 1;
                continue;
            }
        }
        let ch = html[i..].chars().next().expect("utf8");
        out.push(ch);
        i += ch.len_utf8();
    }

    if out == html {
        Cow::Borrowed(html)
    } else {
        Cow::Owned(out)
    }
}

fn is_align_center_div_open(tag: &str) -> bool {
    if !is_div_open_tag(tag) {
        return false;
    }
    let lower = tag.to_ascii_lowercase();
    lower.contains("align=\"center\"")
        || lower.contains("align='center'")
        || lower.contains("align=\"centre\"")
        || lower.contains("align='centre'")
        || lower.contains("align=center")
        || lower.contains("align=centre")
}

fn is_div_open_tag(tag: &str) -> bool {
    let lower = tag.trim_start().to_ascii_lowercase();
    lower.starts_with("<div")
        && matches!(
            lower.as_bytes().get(4).copied(),
            None | Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        )
}

fn is_div_close_tag(tag: &str) -> bool {
    let lower = tag.trim_start().to_ascii_lowercase();
    lower.starts_with("</div")
        && matches!(
            lower.as_bytes().get(5).copied(),
            None | Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        )
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

#[cfg(feature = "_html_preprocess")]
fn apply_lol_html_rewrites(html: &str) -> Result<String, lol_html::errors::RewritingError> {
    use lol_html::{RewriteStrSettings, element, rewrite_str};

    const EVENT_ATTRS: &[&str] = &[
        "onclick",
        "ondblclick",
        "onmousedown",
        "onmouseup",
        "onmouseover",
        "onmouseout",
        "onkeydown",
        "onkeyup",
        "onkeypress",
        "onload",
        "onerror",
        "onfocus",
        "onblur",
    ];

    rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: vec![
                element!("*", |el| {
                    for attr in EVENT_ATTRS {
                        if el.get_attribute(attr).is_some() {
                            el.remove_attribute(attr);
                        }
                    }
                    Ok(())
                }),
                element!("img:not([loading])", |el| {
                    el.set_attribute("loading", "lazy").ok();
                    Ok(())
                }),
                element!("a[href^='http:']", |el| {
                    if let Some(href) = el.get_attribute("href") {
                        el.set_attribute("href", &href.replacen("http:", "https:", 1))
                            .ok();
                    }
                    Ok(())
                }),
            ],
            ..RewriteStrSettings::new()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div_align_center_becomes_center_element() {
        let out = normalize_legacy_alignment_wrappers("<div align=\"center\"><h1>Title</h1></div>");
        assert_eq!(out, "<center><h1>Title</h1></center>");
    }

    #[test]
    fn nested_plain_div_does_not_close_alignment_wrapper() {
        let out = normalize_legacy_alignment_wrappers(
            "<div align=\"center\">Привет<div>inner</div>after</div>",
        );
        assert_eq!(out, "<center>Привет<div>inner</div>after</center>");
    }

    #[test]
    fn nested_alignment_wrappers_keep_their_own_closing_tags() {
        let out = normalize_legacy_alignment_wrappers(
            "<div align=center><div align=centre>x</div></div>",
        );
        assert_eq!(out, "<center><center>x</center></center>");
    }

    #[test]
    fn div_attributes_with_gt_keep_their_original_tag_boundary() {
        let source = "<div data-value=\">\"><span>www.example.com</span></div>";
        assert_eq!(normalize_legacy_alignment_wrappers(source), source);
    }

    #[cfg(feature = "_html_preprocess")]
    #[test]
    fn rewrite_is_deterministic() {
        let input = "<a href=\"http://ex.com\">x</a><img src=\"a.png\">";
        let a = preprocess_raw_html(input);
        let b = preprocess_raw_html(input);
        assert_eq!(a, b);
    }

    #[cfg(feature = "_html_preprocess")]
    #[test]
    fn img_gets_lazy_loading() {
        let out = preprocess_raw_html("<img src=\"x.png\">");
        assert!(out.contains("loading=\"lazy\""), "out: {out}");
    }

    #[cfg(feature = "_html_preprocess")]
    #[test]
    fn strips_inline_event_handlers() {
        let out = preprocess_raw_html("<button onclick=\"alert(1)\">x</button>");
        assert!(!out.contains("onclick"), "out: {out}");
    }

    #[cfg(feature = "_html_preprocess")]
    #[test]
    fn upgrades_insecure_links() {
        let out = preprocess_raw_html("<a href=\"http://example.com\">link</a>");
        assert!(out.contains("https://example.com"), "out: {out}");
    }
}
