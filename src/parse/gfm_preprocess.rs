//! GFM extensions not provided by pulldown-cmark (extended autolinks, spec §6.9).

use std::borrow::Cow;

/// Rewrite `www.` / bare-email autolinks into bracketed `http` autolinks pulldown understands.
#[must_use]
pub fn apply_gfm_extended_autolinks(source: &str) -> Cow<'_, str> {
    if !source.contains("www.") && !source.contains('@') {
        return Cow::Borrowed(source);
    }

    let mut out = String::with_capacity(source.len() + 32);
    let mut fence = None;
    let mut html_block = None;
    let mut code_span_ticks = None;
    for line in source.split_inclusive('\n') {
        if let Some(active) = fence {
            out.push_str(line);
            if fence_closes(line, active) {
                fence = None;
            }
            continue;
        }

        if let Some(active) = html_block {
            out.push_str(line);
            html_block = html_block_state_after_line(line, active);
            continue;
        }

        if let Some(start) = fence_start(line) {
            out.push_str(line);
            fence = Some(start);
            continue;
        }

        if is_indented_code_line(line) {
            out.push_str(line);
            continue;
        }

        if let Some(start) = html_block_start(line) {
            out.push_str(line);
            html_block = html_block_state(line, start);
            continue;
        }

        out.push_str(&rewrite_line(line, &mut code_span_ticks));
    }
    Cow::Owned(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fence {
    marker: u8,
    len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlBlock {
    Comment,
    Tag { name: &'static str, depth: usize },
}

fn fence_start(line: &str) -> Option<Fence> {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && i < 3 && bytes[i] == b' ' {
        i += 1;
    }
    let marker = *bytes.get(i)?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let start = i;
    while i < bytes.len() && bytes[i] == marker {
        i += 1;
    }
    let len = i - start;
    (len >= 3).then_some(Fence { marker, len })
}

fn fence_closes(line: &str, fence: Fence) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && i < 3 && bytes[i] == b' ' {
        i += 1;
    }
    let start = i;
    while i < bytes.len() && bytes[i] == fence.marker {
        i += 1;
    }
    if i - start < fence.len {
        return false;
    }
    line[i..].trim().is_empty()
}

fn is_indented_code_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.first() == Some(&b'\t') || bytes.get(..4) == Some(b"    ")
}

fn html_block_start(line: &str) -> Option<HtmlBlock> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("<!--") {
        return Some(HtmlBlock::Comment);
    }
    if trimmed.starts_with("<![CDATA[") || trimmed.starts_with("<?") {
        return Some(HtmlBlock::Comment);
    }
    let bytes = trimmed.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1usize;
    if bytes.get(i) == Some(&b'/') {
        return None;
    }
    let start = i;
    while let Some(byte) = bytes.get(i).copied() {
        if !byte.is_ascii_alphabetic() {
            break;
        }
        i += 1;
    }
    if i == start {
        return None;
    }
    let name = &trimmed[start..i];
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "base",
        "basefont",
        "blockquote",
        "body",
        "caption",
        "center",
        "col",
        "colgroup",
        "dd",
        "details",
        "dialog",
        "dir",
        "div",
        "dl",
        "dt",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hr",
        "html",
        "iframe",
        "legend",
        "li",
        "link",
        "main",
        "menu",
        "menuitem",
        "nav",
        "ol",
        "p",
        "pre",
        "script",
        "section",
        "summary",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "title",
        "tr",
        "track",
        "ul",
        "style",
        "textarea",
        "xmp",
    ];
    BLOCK_TAGS
        .iter()
        .copied()
        .find(|tag| name.eq_ignore_ascii_case(tag))
        .map(|name| HtmlBlock::Tag { name, depth: 1 })
}

fn html_block_state(line: &str, block: HtmlBlock) -> Option<HtmlBlock> {
    match block {
        HtmlBlock::Comment => {
            if line.contains("-->") || line.contains("]]>") || line.contains("?>") {
                None
            } else {
                Some(block)
            }
        }
        HtmlBlock::Tag { name, .. } => {
            let depth = html_tag_balance(line, name, true);
            (depth > 0).then_some(HtmlBlock::Tag {
                name,
                depth: depth as usize,
            })
        }
    }
}

fn html_block_state_after_line(line: &str, block: HtmlBlock) -> Option<HtmlBlock> {
    match block {
        HtmlBlock::Comment => html_block_state(line, block),
        HtmlBlock::Tag { name, depth } => {
            if !html_block_requires_closing(name) && line.trim().is_empty() {
                return None;
            }
            let delta = html_tag_balance(line, name, !is_raw_text_html_tag(name));
            let depth = if delta < 0 {
                depth.saturating_sub((-delta) as usize)
            } else {
                depth.saturating_add(delta as usize)
            };
            (depth > 0).then_some(HtmlBlock::Tag { name, depth })
        }
    }
}

fn html_tag_balance(line: &str, name: &str, count_openings: bool) -> isize {
    let lower = line.to_ascii_lowercase();
    let opening = format!("<{name}");
    let closing = format!("</{name}");
    let mut balance = 0isize;
    let mut cursor = 0usize;
    while let Some(relative) = lower[cursor..].find(&opening) {
        let position = cursor + relative;
        let after_name = position + opening.len();
        if count_openings
            && tag_name_terminator(lower.as_bytes().get(after_name).copied())
            && !is_self_closing_tag(&line[position..])
            && !is_void_html_tag(name)
        {
            balance += 1;
        }
        cursor = after_name;
    }
    cursor = 0;
    while let Some(relative) = lower[cursor..].find(&closing) {
        let position = cursor + relative;
        let after_name = position + closing.len();
        if tag_name_terminator(lower.as_bytes().get(after_name).copied()) {
            balance -= 1;
        }
        cursor = after_name;
    }
    balance
}

fn tag_name_terminator(next: Option<u8>) -> bool {
    matches!(
        next,
        None | Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
}

fn is_void_html_tag(name: &str) -> bool {
    matches!(
        name,
        "base"
            | "col"
            | "embed"
            | "hr"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "basefont"
    )
}

fn is_raw_text_html_tag(name: &str) -> bool {
    matches!(name, "script" | "style" | "pre" | "textarea" | "xmp")
}

fn html_block_requires_closing(name: &str) -> bool {
    is_raw_text_html_tag(name)
}

fn is_self_closing_tag(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut quote = None;
    for (index, byte) in bytes.iter().copied().enumerate().skip(1) {
        match quote {
            Some(active) if byte == active => quote = None,
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => {
                return source[..index].trim_end().ends_with('/');
            }
            _ => {}
        }
    }
    false
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

fn rewrite_line(line: &str, code_span_ticks: &mut Option<usize>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len() + 16);
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(active_ticks) = *code_span_ticks {
            if bytes[i] == b'`' {
                let start = i;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                out.push_str(&line[start..i]);
                if i - start == active_ticks {
                    *code_span_ticks = None;
                }
                continue;
            }
            let ch = line[i..].chars().next().expect("utf8");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }

        if bytes[i] == b'`' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            out.push_str(&line[start..i]);
            *code_span_ticks = Some(i - start);
            continue;
        }

        if bytes[i] == b'<'
            && let Some(tag_end) = html_tag_end(&line[i..])
        {
            let end = i + tag_end + 1;
            let tag = &line[i..end];
            if looks_like_html_tag(tag) {
                out.push_str(tag);
                i = end;
                continue;
            }
        }

        if let Some((len, replacement)) = try_www_autolink(line, i) {
            out.push_str(&replacement);
            i += len;
            continue;
        }
        if let Some(scan) = scan_bare_email_candidate(line, i) {
            match scan {
                EmailScan::Rewritten {
                    consumed,
                    replacement,
                } => {
                    out.push_str(&replacement);
                    i += consumed;
                    continue;
                }
                EmailScan::Original { consumed } if consumed > 0 => {
                    out.push_str(&line[i..i + consumed]);
                    i += consumed;
                    continue;
                }
                EmailScan::Original { .. } => {}
            }
        }
        let ch = line[i..].chars().next().expect("utf8");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn looks_like_html_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'<' {
        return false;
    }
    let mut i = 1usize;
    if bytes[i] == b'/' {
        i += 1;
    }
    if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let start = i;
    while let Some(byte) = bytes.get(i).copied() {
        if !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b':') {
            break;
        }
        i += 1;
    }
    i > start
}

fn autolink_may_start(line: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    if inside_markdown_link_destination(line, start) {
        return false;
    }
    let prev = line.as_bytes()[start - 1];
    prev.is_ascii_whitespace() || matches!(prev, b'*' | b'_' | b'~' | b'(')
}

fn inside_markdown_link_destination(line: &str, start: usize) -> bool {
    if start < 2 {
        return false;
    }
    let bytes = line.as_bytes();
    bytes[start - 1] == b'(' && bytes[start - 2] == b']'
}

enum EmailScan {
    Rewritten {
        consumed: usize,
        replacement: String,
    },
    Original {
        consumed: usize,
    },
}

fn try_www_autolink(line: &str, start: usize) -> Option<(usize, String)> {
    if !autolink_may_start(line, start) {
        return None;
    }
    let rest = &line[start..];
    if !rest.starts_with("www.") {
        return None;
    }
    let domain_end = www_domain_end(rest)?;
    let path_end = www_path_end(&rest[..domain_end], &rest[domain_end..]);
    let full_end = domain_end + path_end;
    let link_text = &rest[..full_end];
    let trimmed = trim_trailing_punctuation(link_text);
    if trimmed.is_empty() {
        return None;
    }
    let suffix = &link_text[trimmed.len()..];
    let href = format!("http://{trimmed}");
    let replacement = format!("<{href}>{suffix}");
    Some((full_end, replacement))
}

fn www_domain_end(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.len() < 5 || !rest.starts_with("www.") {
        return None;
    }
    let mut i = 4usize;
    let mut period_count = 0usize;
    let mut since_period = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
            since_period += 1;
            i += 1;
            continue;
        }
        if c == b'.' {
            if i == 4 || since_period == 0 {
                return None;
            }
            let next = bytes.get(i + 1).copied();
            if !matches!(
                next,
                Some(next) if next.is_ascii_alphanumeric() || next == b'_' || next == b'-'
            ) {
                break;
            }
            period_count += 1;
            since_period = 0;
            i += 1;
            continue;
        }
        break;
    }
    if period_count == 0 || since_period == 0 {
        return None;
    }
    let last_two = rest[4..i].split('.').rev().take(2).collect::<Vec<_>>();
    if last_two.iter().any(|seg| seg.contains('_')) {
        return None;
    }
    Some(i)
}

fn www_path_end(domain: &str, tail: &str) -> usize {
    let mut len = 0usize;
    for ch in tail.chars() {
        if ch.is_whitespace()
            || ch == '<'
            || "。．，、？！：；（）-【】「」『』〈〉《》".contains(ch)
        {
            break;
        }
        len += ch.len_utf8();
    }
    let _ = domain;
    len
}

fn trim_trailing_punctuation(link: &str) -> &str {
    let mut end = link.len();
    while end > 0 {
        let ch = link[..end].chars().last().unwrap();
        if "?!.:,*_~。．，、？！：；（）-【】「」『』〈〉《》".contains(ch) {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    if link.as_bytes().get(end - 1) == Some(&b')') {
        let open = link[..end].chars().filter(|&c| c == '(').count();
        let close = link[..end].chars().filter(|&c| c == ')').count();
        if close > open {
            while end > 0 && link.as_bytes()[end - 1] == b')' {
                end -= 1;
            }
        }
    }
    &link[..end]
}

fn scan_bare_email_candidate(line: &str, start: usize) -> Option<EmailScan> {
    if !autolink_may_start(line, start) {
        return None;
    }
    let rest = &line[start..];
    if !rest.chars().next().is_some_and(is_email_local_char) {
        return None;
    }

    let mut at = None;
    for (offset, ch) in rest.char_indices() {
        if ch == '@' {
            at = Some(offset);
            break;
        }
        if ch.is_whitespace() || ch == '<' {
            return Some(EmailScan::Original { consumed: offset });
        }
        if !is_email_local_char(ch) {
            return Some(EmailScan::Original { consumed: offset });
        }
    }

    let Some(at) = at else {
        return Some(EmailScan::Original {
            consumed: rest.len(),
        });
    };

    let domain_len = email_domain_end(&rest[at + 1..])?;
    let email = &rest[..at + 1 + domain_len];
    let replacement = format!("<mailto:{email}>");
    Some(EmailScan::Rewritten {
        consumed: email.len(),
        replacement,
    })
}

fn is_email_local_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(ch)
}

fn email_domain_end(domain: &str) -> Option<usize> {
    let mut i = 0usize;
    let mut label_start = 0usize;
    let mut labels = 0usize;
    while i < domain.len() {
        let b = domain.as_bytes()[i];
        if b.is_ascii_alphanumeric() || b == b'-' {
            i += 1;
            continue;
        }
        if b == b'.' {
            if i == label_start {
                return None;
            }
            labels += 1;
            label_start = i + 1;
            i += 1;
            continue;
        }
        break;
    }
    if labels == 0 || i == label_start {
        return None;
    }
    Some(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_line_becomes_bracketed_http() {
        let out = apply_gfm_extended_autolinks("www.commonmark.org\n");
        assert!(out.contains("<http://www.commonmark.org>"));
    }

    #[test]
    fn www_in_sentence() {
        let out = apply_gfm_extended_autolinks("Visit www.commonmark.org/help for more.\n");
        assert!(out.contains("<http://www.commonmark.org/help>"));
    }

    #[test]
    fn trailing_punctuation_is_excluded_from_www_link() {
        let out = apply_gfm_extended_autolinks("Trailing punctuation excluded: www.example.com.\n");
        assert!(out.contains("<http://www.example.com>."));
    }

    #[test]
    fn bare_email_becomes_mailto_autolink() {
        let out = apply_gfm_extended_autolinks("mail me at user.name+tag@example.com\n");
        assert!(out.contains("<mailto:user.name+tag@example.com>"));
    }

    #[test]
    fn invalid_local_prefix_does_not_block_later_email_start() {
        let out = apply_gfm_extended_autolinks("a(user@example.com)\n");
        assert!(out.contains("a(<mailto:user@example.com>)"));
    }

    #[test]
    fn relative_markdown_link_destinations_are_not_rewritten_as_autolinks() {
        let out = apply_gfm_extended_autolinks(
            "[![PT-BR](https://img.shields.io/badge/PT--BR-README-green)](README.pt-BR.md)\n",
        );
        assert!(out.contains("](README.pt-BR.md)"));
        assert!(!out.contains("http://README.pt-BR.md"));
        assert!(!out.contains("http://readme.pt-br.md"));
    }

    #[test]
    fn trailing_cjk_punctuation_is_excluded_from_www_link() {
        let out = apply_gfm_extended_autolinks(
            "Посети www.example.com。 или www.example.org，чтобы узнать больше。\n",
        );
        assert!(out.contains("<http://www.example.com>。"));
        assert!(out.contains("<http://www.example.org>，"));
    }

    #[test]
    fn fenced_code_is_not_rewritten() {
        let source = "```text\nwww.example.com user@example.com\n```\n";
        assert_eq!(apply_gfm_extended_autolinks(source), source);
    }

    #[test]
    fn indented_code_is_not_rewritten() {
        let source = "    www.example.com user@example.com\n";
        assert_eq!(apply_gfm_extended_autolinks(source), source);
    }

    #[test]
    fn raw_html_block_is_not_rewritten() {
        let source = "<div>\nwww.example.com user@example.com\n</div>\n";
        assert_eq!(apply_gfm_extended_autolinks(source), source);
    }

    #[test]
    fn nested_html_blocks_stay_protected_until_outer_close() {
        let source = "<div>\n<div>inner</div>\nwww.example.com user@example.com\n</div>\n";
        assert_eq!(apply_gfm_extended_autolinks(source), source);
    }

    #[test]
    fn similar_closing_tag_does_not_end_html_block() {
        let source = "<div>\n</diverse>\nwww.example.com user@example.com\n</div>\n";
        assert_eq!(apply_gfm_extended_autolinks(source), source);
    }

    #[test]
    fn void_html_block_does_not_swallow_following_markdown() {
        let source = "<hr>\nwww.example.com user@example.com\n";
        let rewritten = apply_gfm_extended_autolinks(source);
        assert!(rewritten.contains("<http://www.example.com>"));
    }

    #[test]
    fn unclosed_block_html_ends_at_blank_line() {
        let source = "<div>\nwww.example.com\n\nwww.example.org\n";
        let rewritten = apply_gfm_extended_autolinks(source);
        assert!(rewritten.contains("www.example.com\n"));
        assert!(rewritten.contains("<http://www.example.org>"));
    }

    #[test]
    fn inline_html_attributes_with_gt_are_not_rewritten() {
        let source = r#"<span title="www.example.com > user@example.com">text</span>"#;
        assert_eq!(apply_gfm_extended_autolinks(source), source);
    }
}
