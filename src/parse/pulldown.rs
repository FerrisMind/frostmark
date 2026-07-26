use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::core::block::{BlockContent, BlockKind, BlockStatus, RenderBlock};
use crate::core::error::ParseError;
use crate::core::ids::BlockId;
use crate::html::sanitize;
use crate::options::ParseOptions;
use crate::parse::content::{
    block_content_from_events, code_text_from_events, is_code_fence_slice,
};
use crate::parse::gfm_preprocess;
use crate::parse::wrapper_coalesce::{
    events_to_html, extend_through_unclosed_container, html_event_extent,
    is_coalesced_wrapper_block, starts_unclosed_html_container,
};
use crate::profile::ParseProfile;

/// Collect pulldown events into backend-agnostic [`RenderBlock`] values.
pub fn parse_blocks(
    source: &str,
    profile: ParseProfile,
    options: &ParseOptions,
) -> Result<Vec<RenderBlock>, ParseError> {
    let prepared = if profile.uses_gfm_extensions() {
        gfm_preprocess::apply_gfm_extended_autolinks(source)
    } else {
        source.into()
    };
    let parser = Parser::new_ext(&prepared, options.pulldown);
    let (events, ranges): (Vec<_>, Vec<_>) = parser
        .into_offset_iter()
        .map(|(event, range)| (event.into_static(), range))
        .unzip();

    Ok(group_events_into_blocks(
        prepared.as_ref(),
        events,
        ranges,
        options.raw_html,
        options.gfm_tagfilter,
    ))
}

fn group_events_into_blocks(
    source: &str,
    events: Vec<Event<'static>>,
    ranges: Vec<Range<usize>>,
    raw_html: crate::options::RawHtmlPolicy,
    gfm_tagfilter: bool,
) -> Vec<RenderBlock> {
    let source_arc = Arc::<str>::from(source);
    let mut blocks = Vec::new();
    let mut next_id = 1u64;
    let mut index = 0usize;

    while index < events.len() {
        let (kind, rel_end) = classify_block_start(&events[index..]);
        let mut rel_end = rel_end.max(1);
        if starts_unclosed_html_container(&events[index..index + rel_end]) {
            let abs_end = extend_through_unclosed_container(&events, index, index + rel_end);
            rel_end = abs_end - index;
        }
        let end = index + rel_end;
        let slice = &events[index..end];
        let range_slice = &ranges[index..end];
        let coalesced = is_coalesced_wrapper_block(slice);
        let coalesced_html = coalesced.then(|| events_to_html(slice));
        let block_source = if is_code_fence_slice(slice) {
            Arc::<str>::from(code_text_from_events(slice))
        } else if let Some(html) = &coalesced_html {
            Arc::<str>::from(html.as_str())
        } else {
            Arc::<str>::from(event_slice_source(source, range_slice))
        };
        let content = if let Some(html) = &coalesced_html {
            sanitize::block_content_from_raw_html(html, raw_html, gfm_tagfilter)
        } else if kind == BlockKind::MathBlock {
            #[cfg(feature = "math")]
            {
                if let Some(latex) = crate::parse::content::display_math_latex_from_events(slice) {
                    BlockContent::Math {
                        latex,
                        display: true,
                    }
                } else {
                    block_content_from_events(slice, block_source.clone(), raw_html, gfm_tagfilter)
                }
            }
            #[cfg(not(feature = "math"))]
            {
                block_content_from_events(slice, block_source.clone(), raw_html, gfm_tagfilter)
            }
        } else {
            block_content_from_events(slice, block_source.clone(), raw_html, gfm_tagfilter)
        };
        let kind = if coalesced {
            BlockKind::HtmlBlock
        } else {
            kind
        };

        blocks.push(RenderBlock {
            id: BlockId::new(next_id),
            status: BlockStatus::Committed,
            kind,
            source: block_source,
            content,
        });
        next_id += 1;
        index = end;
    }

    if blocks.is_empty() && !source.is_empty() {
        blocks.push(RenderBlock {
            id: BlockId::new(next_id),
            status: BlockStatus::Committed,
            kind: BlockKind::Paragraph,
            source: source_arc.clone(),
            content: BlockContent::Markdown(crate::core::block::CompiledMarkdown::new(
                source_arc, events,
            )),
        });
    }

    blocks
}

fn classify_block_start(events: &[Event<'static>]) -> (BlockKind, usize) {
    let Some(first) = events.first() else {
        return (BlockKind::Unknown, 0);
    };

    match first {
        Event::Start(tag) => {
            let len = block_extent_for_tag(events, tag);
            (tag_to_kind(tag), len)
        }
        Event::Html(_) | Event::InlineHtml(_) => (BlockKind::HtmlBlock, html_event_extent(events)),
        Event::Rule => (BlockKind::ThematicBreak, 1),
        Event::FootnoteReference(_) => (BlockKind::FootnoteDefinition, 1),
        Event::DisplayMath(_) => (BlockKind::MathBlock, 1),
        _ => (BlockKind::Paragraph, paragraph_extent(events)),
    }
}

fn tag_to_kind(tag: &Tag<'_>) -> BlockKind {
    match tag {
        Tag::Paragraph => BlockKind::Paragraph,
        Tag::Heading { .. } => BlockKind::Heading,
        Tag::CodeBlock(_) => BlockKind::CodeFence,
        Tag::List(_) => BlockKind::List,
        Tag::BlockQuote(_) => BlockKind::BlockQuote,
        Tag::Table(_) => BlockKind::Table,
        Tag::HtmlBlock => BlockKind::HtmlBlock,
        Tag::FootnoteDefinition(_) => BlockKind::FootnoteDefinition,
        _ => BlockKind::Unknown,
    }
}

fn block_extent_for_tag(events: &[Event<'static>], tag: &Tag<'_>) -> usize {
    if matches!(tag, Tag::HtmlBlock) {
        return events
            .iter()
            .position(|event| matches!(event, Event::End(TagEnd::HtmlBlock)))
            .map(|i| i + 1)
            .unwrap_or(events.len());
    }

    let end_tag: TagEnd = tag.clone().into();
    let mut depth = 0usize;
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::Start(t) if same_container_start(t, tag) => depth += 1,
            Event::End(t) if same_container_end(*t, end_tag) => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    events.len()
}

fn same_container_start(candidate: &Tag<'_>, expected: &Tag<'_>) -> bool {
    match (candidate, expected) {
        (Tag::List(_), Tag::List(_)) => true,
        _ => candidate == expected,
    }
}

fn same_container_end(candidate: TagEnd, expected: TagEnd) -> bool {
    match (candidate, expected) {
        (TagEnd::List(_), TagEnd::List(_)) => true,
        _ => candidate == expected,
    }
}

fn paragraph_extent(events: &[Event<'static>]) -> usize {
    for (i, event) in events.iter().enumerate().skip(1) {
        if matches!(
            event,
            Event::Start(Tag::Paragraph)
                | Event::Start(Tag::Heading { .. })
                | Event::Start(Tag::CodeBlock(_))
                | Event::Start(Tag::List(_))
                | Event::Html(_)
                | Event::Rule
        ) {
            return i;
        }
    }
    events.len()
}

fn event_slice_source(source: &str, ranges: &[Range<usize>]) -> String {
    let Some(start) = ranges.first().map(|range| range.start) else {
        return String::new();
    };
    let end = ranges
        .last()
        .map(|range| range.end)
        .unwrap_or(start)
        .min(source.len());
    source[start.min(end)..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::ParseOptions;

    #[test]
    fn github_preview_enables_expected_options() {
        let opts = ParseOptions::for_profile(ParseProfile::GitHubPreview);
        let expected = ParseProfile::GitHubPreview.pulldown_options();
        assert_eq!(opts.pulldown, expected);
        use pulldown_cmark::Options;
        assert!(opts.pulldown.contains(Options::ENABLE_TABLES));
        assert!(opts.pulldown.contains(Options::ENABLE_TASKLISTS));
        assert!(opts.pulldown.contains(Options::ENABLE_STRIKETHROUGH));
        assert!(opts.pulldown.contains(Options::ENABLE_FOOTNOTES));
        assert!(opts.pulldown.contains(Options::ENABLE_GFM));
        assert!(opts.pulldown.contains(Options::ENABLE_MATH));
        assert!(opts.pulldown.contains(Options::ENABLE_WIKILINKS));
    }

    #[cfg(feature = "math")]
    #[test]
    fn display_math_paragraph_maps_to_math_content() {
        let source = "Display:\n\n$$\\frac{1}{2}$$\n";
        let blocks = parse_blocks(
            source,
            ParseProfile::GitHubPreview,
            &ParseOptions::for_profile(ParseProfile::GitHubPreview),
        )
        .expect("parse");
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b.content, BlockContent::Math { .. }))
        );
    }

    #[test]
    fn parses_paragraph_and_heading_blocks() {
        let source = "# Title\n\nHello **world**.";
        let blocks = parse_blocks(
            source,
            ParseProfile::GitHubPreview,
            &ParseOptions::default(),
        )
        .expect("parse");
        assert!(blocks.len() >= 2);
        assert!(blocks.iter().any(|b| b.kind == BlockKind::Heading));
        assert!(blocks.iter().any(|b| b.kind == BlockKind::Paragraph));
    }

    #[test]
    fn block_source_uses_event_offsets_not_full_document() {
        let source = "# Title\n\nHello **world**.\n\n- one\n- two\n";
        let blocks = parse_blocks(
            source,
            ParseProfile::GitHubPreview,
            &ParseOptions::default(),
        )
        .expect("parse");
        let paragraph = blocks
            .iter()
            .find(|block| block.kind == BlockKind::Paragraph)
            .expect("paragraph");
        assert_eq!(paragraph.source.trim_end(), "Hello **world**.");
        assert_ne!(paragraph.source.as_ref(), source);
    }

    #[test]
    fn ordered_list_extent_includes_nested_list_with_different_start() {
        let source = "1. outer\n   3. nested\n2. after\n\ntrailing\n";
        let parser = Parser::new_ext(source, pulldown_cmark::Options::all());
        let events: Vec<_> = parser.map(|event| event.into_static()).collect();
        let Event::Start(tag) = &events[0] else {
            panic!("expected list start, got {:?}", events.first());
        };
        assert!(matches!(tag, Tag::List(Some(1))));
        let extent = block_extent_for_tag(&events, tag);
        assert!(extent > 0);
        assert!(matches!(events[extent - 1], Event::End(TagEnd::List(_))));
        assert!(matches!(
            events.get(extent),
            Some(Event::Start(Tag::Paragraph))
        ));
    }

    #[test]
    fn multiline_raw_html_file_is_single_block() {
        let source = "<details>\n<summary>x</summary>\n</details>\n";
        let blocks = parse_blocks(
            source,
            ParseProfile::GitHubPreview,
            &ParseOptions::default(),
        )
        .expect("parse");
        assert_eq!(blocks.len(), 1, "blocks: {:?}", blocks.len());
        #[cfg(feature = "static")]
        {
            use crate::html::writer;
            let html = writer::blocks_to_html(&blocks).expect("html");
            assert!(html.contains("summary"), "html: {html}");
        }
    }

    #[test]
    fn routes_raw_html_to_fragment() {
        let source = "<details><summary>x</summary></details>";
        let blocks = parse_blocks(
            source,
            ParseProfile::GitHubPreview,
            &ParseOptions::default(),
        )
        .expect("parse");
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b.content, BlockContent::Html(_)))
        );
    }

    #[test]
    #[ignore = "manual HTML probe; not part of CI"]
    fn print_math_events() {
        let source1 = "$$\\frac{1}{2}$$";
        println!("--- source1: {source1:?} ---");
        let parser1 = pulldown_cmark::Parser::new_ext(
            source1,
            ParseProfile::GitHubPreview.pulldown_options(),
        );
        let mut html1 = String::new();
        pulldown_cmark::html::push_html(&mut html1, parser1);
        println!("HTML1: {html1}");

        let source2 = "$$\n\\frac{1}{2}\n$$";
        println!("--- source2: {source2:?} ---");
        let parser2 = pulldown_cmark::Parser::new_ext(
            source2,
            ParseProfile::GitHubPreview.pulldown_options(),
        );
        let mut html2 = String::new();
        pulldown_cmark::html::push_html(&mut html2, parser2);
        println!("HTML2: {html2}");
        panic!("force print");
    }
}
