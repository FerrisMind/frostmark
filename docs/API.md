# StriMD public API

This document describes the **stable public contract** for StriMD integrators. The only user-facing Cargo features are `no_iced`, `static`, and `stream`.

Implementation features (`_iced_backend`, `math`, `mermaid`, `_html_preprocess`, `_rcdom_compat`) are migration internals and may change without notice. Default GUI builds enable math and mermaid through `_iced_backend`; do not list `iced/*` passthrough flags in downstream manifests.

## Cargo features

| Feature | Required with | Enables |
|---------|---------------|---------|
| `no_iced` | `default-features = false` | Headless builds without iced/GPU |
| `static` | — | `Document::parse`, `Document::to_html()`, `HtmlFragment` |
| `stream` | — | `StreamDocument`, `StreamPatch`, `StreamOptions` |

Full headless stack (static export + streaming + egui harness parity): enable all three together.

Typical headless dependency:

```toml
strimd = { version = "1.0", default-features = false, features = ["no_iced", "static", "stream"] }
```

Default GUI dependency (iced + pulldown):

```toml
strimd = "1.0"
```

## Core types

All backends share the same block model:

| Type | Role |
|------|------|
| `Document` | Fully parsed static Markdown document |
| `StreamDocument` | Append-only streaming document (LLM tokens) |
| `RenderBlock` | One renderable unit (paragraph, heading, table, HTML, …) |
| `BlockId` | Stable block identifier for stream patches |
| `BlockKind` | Block classification (`Paragraph`, `Table`, `HtmlBlock`, …) |
| `BlockContent` | Payload: Markdown events cache or `HtmlFragment` |
| `BlockStatus` | Committed vs pending (streaming) |
| `ParseProfile` | Parser preset (`GitHubPreview`, `ChatStream`, …) |
| `ParseOptions` | Fine-grained parse policy (raw HTML, pulldown options) |

Pulldown event internals are **not** exposed in the public API.

## Static preview

```rust
use strimd::{Document, ParseProfile};

let doc = Document::parse(source, ParseProfile::GitHubPreview)?;
assert_eq!(doc.parse_backend(), strimd::ParseBackend::Pulldown);

for block in doc.blocks() {
    // inspect block.kind, block.content, block.id
}

#[cfg(feature = "static")]
let html = doc.to_html()?;
```

`ParseProfile::GitHubPreview` enables GFM tables, task lists, strikethrough, footnotes, alerts, math, and wikilinks (pulldown-cmark 0.13).

### Diagnostics

`Document::diagnostics()` reports the active parser backend (always `ParseBackend::Pulldown`).

```rust
let doc = Document::parse(source, ParseProfile::GitHubPreview)?;
assert_eq!(doc.parse_backend(), strimd::ParseBackend::Pulldown);
assert_eq!(doc.diagnostics().to_string(), "backend=pulldown");
```

## LLM streaming

Use `StreamOptions::chat()` for chat UIs. It configures mdstream to invalidate footnotes and late reference definitions.

```rust
use strimd::{StreamDocument, StreamOptions, StreamPatch};

let mut doc = StreamDocument::new(StreamOptions::chat());

for chunk in token_chunks {
    let update = doc.append(chunk);
    if update.reset {
        // rebuild UI from scratch
    }
    match update.patch {
        StreamPatch::AppendCommitted { blocks } => { /* append new blocks */ }
        StreamPatch::ReplacePending => { /* refresh tail */ }
        StreamPatch::ReplaceCommitted { id } => { /* reparse invalidated block */ }
        StreamPatch::ClearAndRebuild => { /* full rebuild */ }
        StreamPatch::Noop => {}
    }
}

doc.finalize();
```

Committed blocks are parsed once and cached. Pending blocks reparse on each append. Raw HTML in streams follows the same `HtmlFragment` path as static parsing.

## Iced backend (default builds)

When `_iced_backend` is active (default, without `no_iced`):

```rust
use strimd::{Document, MarkState, MarkWidget, ParseProfile};

// Legacy string API (still supported)
let state = MarkState::with_html_and_markdown(text);

// Preferred: shared core document
let doc = Document::parse(text, ParseProfile::GitHubPreview)?;
let state = MarkState::from_document(&doc);

// In view:
// MarkWidget::new(&state)
```

Streaming UI:

```rust
#[cfg(feature = "stream")]
{
    let mut stream = StreamDocument::new(StreamOptions::chat());
    stream.append(chunk);
    let mut state = MarkState::from_document(/* or from_blocks */);
    state.sync_from_stream(&stream);
}
```

### Math and Mermaid (default iced backend)

With `default-features = true`, the `_iced_backend` bundle enables LaTeX and Mermaid rendering:

- **Display math** (`$$…$$` / `MathBlock`) and **inline math** (`$…$`, DOM `span.math-inline`) via RaTeX → self-contained SVG (`embed-fonts`).
- **Mermaid** fenced blocks (` ```mermaid `) via `mermaid-rs-renderer` when the fence is complete (streaming shows source until the closing fence).

Low-level SVG helpers (crate tests / custom UI; not part of the three-feature public contract):

```rust
#[cfg(feature = "math")]
use strimd::render::latex_to_svg;

#[cfg(feature = "mermaid")]
use strimd::render::mermaid_to_svg;
```

`Document::to_html()` still emits placeholder `<span class="math">` / diagram stubs; full SVG export is not in scope for headless builds.

## HTML fragments

Raw HTML in Markdown is parsed into owned `HtmlFragment` trees via html5ever TreeSink (not RcDom in production):

```rust
#[cfg(feature = "static")]
use strimd::HtmlFragment;

let fragment = HtmlFragment::from_html("<details><summary>x</summary></details>");
```

Supported tags and sanitization policy are enforced in the iced renderer and `RawHtmlPolicy` (see `ParseOptions`).

## Error types

| Type | When |
|------|------|
| `ParseError` | Markdown parse failure |
| `HtmlFragmentError` | HTML fragment parse failure |
| `RenderError` | HTML export / render failure |
| `UnsupportedReason` | Unsupported construct (safe placeholder) |

Functions return `Result` instead of panicking on malformed input.

## Examples

| Example | Command |
|---------|---------|
| Hello (iced) | `cargo run --example hello` |
| Math + Mermaid | `cargo run --example math_mermaid` |
| Static export | `cargo run --example static_export --no-default-features --features no_iced,static` |
| Stream chat | `cargo run --example stream_chat --no-default-features --features no_iced,stream` |

## Related docs

- [MIGRATION.md](MIGRATION.md) — stack migration guide
- [README_HEADLESS.md](README_HEADLESS.md) — headless quick start (included in crate docs for `no_iced` builds)
- [LEGACY_REMOVAL_GATE.md](LEGACY_REMOVAL_GATE.md) — comrak/RcDom removal criteria
- [CRATE_SPLIT_SPIKE.md](CRATE_SPLIT_SPIKE.md) — single-crate vs split evaluation
