//! Downstream LLM streaming integration (Task 7.2).

#![cfg(all(feature = "no_iced", feature = "stream"))]

use strimd::{BlockContent, StreamDocument, StreamOptions, StreamPatch};

#[test]
fn stream_patches_update_incrementally() {
    let mut doc = StreamDocument::new(StreamOptions::chat());
    let u1 = doc.append("Hello ");
    assert!(matches!(u1.patch, StreamPatch::ReplacePending));

    let u2 = doc.append("world.\n\n");
    assert!(!u2.reset);
    assert!(
        matches!(u2.patch, StreamPatch::AppendCommitted { .. })
            || matches!(u2.patch, StreamPatch::ReplacePending)
    );

    let committed_after = doc.blocks().count();
    let u3 = doc.append("Second paragraph.\n\n");
    assert!(!u3.reset);
    assert!(doc.blocks().count() >= committed_after);
}

#[test]
fn public_finalize_commits_pending_block() {
    let mut doc = StreamDocument::new(StreamOptions::chat());
    doc.append("A final paragraph without a trailing newline");
    assert!(doc.pending().is_some());

    let update = doc.finalize();

    assert!(matches!(update.patch, StreamPatch::AppendCommitted { .. }));
    assert_eq!(doc.blocks().count(), 1);
    assert!(doc.pending().is_none());
}

#[test]
fn chunked_stream_matches_whole_document_block_count() {
    let source = include_str!("fixtures/stream_table.md");
    let whole = {
        let mut doc = StreamDocument::new(StreamOptions::chat());
        doc.append(source);
        doc.blocks().count()
    };
    let chunked = {
        let mut doc = StreamDocument::new(StreamOptions::chat());
        for chunk in source.as_bytes().chunks(4) {
            doc.append(std::str::from_utf8(chunk).unwrap_or(""));
        }
        doc.blocks().count()
    };
    assert_eq!(whole, chunked);
}

#[test]
fn late_reference_invalidates_or_commits() {
    let mut doc = StreamDocument::new(StreamOptions::chat());
    doc.append("See [link][ref].\n\n");
    let update = doc.append("[ref]: https://example.com\n\n");
    assert!(!update.reset);
    assert!(
        !update.invalidated.is_empty()
            || matches!(update.patch, StreamPatch::AppendCommitted { .. })
    );
}

#[test]
fn streamed_raw_html_uses_fragment_content() {
    let mut doc = StreamDocument::new(StreamOptions::chat());
    for chunk in include_str!("fixtures/raw_details.md").as_bytes().chunks(8) {
        doc.append(std::str::from_utf8(chunk).unwrap_or(""));
    }
    assert!(
        doc.blocks()
            .any(|b| matches!(b.content, BlockContent::Html(_)))
    );
}
