use crate::core::block::RenderBlock;
use crate::core::error::ParseError;
use crate::options::ParseOptions;
use crate::parse::diagnostics::ParseDiagnostics;
use crate::parse::pulldown;
use crate::profile::ParseProfile;

/// A fully parsed Markdown document as backend-agnostic blocks.
#[derive(Debug, Clone)]
pub struct Document {
    blocks: Vec<RenderBlock>,
    profile: ParseProfile,
    #[cfg(feature = "static")]
    gfm_tagfilter: bool,
    diagnostics: ParseDiagnostics,
}

impl Document {
    /// Parse `source` with the given profile.
    pub fn parse(source: &str, profile: ParseProfile) -> Result<Self, ParseError> {
        Self::parse_with_options(source, profile, &ParseOptions::for_profile(profile))
    }

    /// Parse with explicit options.
    pub fn parse_with_options(
        source: &str,
        profile: ParseProfile,
        options: &ParseOptions,
    ) -> Result<Self, ParseError> {
        let blocks = pulldown::parse_blocks(source, profile, options)?;
        Ok(Self {
            blocks,
            profile,
            #[cfg(feature = "static")]
            gfm_tagfilter: options.gfm_tagfilter,
            diagnostics: ParseDiagnostics::pulldown(),
        })
    }

    /// All committed render blocks in document order.
    #[must_use]
    pub fn blocks(&self) -> &[RenderBlock] {
        &self.blocks
    }

    /// Profile used to parse this document.
    #[must_use]
    pub fn profile(&self) -> ParseProfile {
        self.profile
    }

    /// Parse diagnostics: active backend.
    #[must_use]
    pub fn diagnostics(&self) -> ParseDiagnostics {
        self.diagnostics
    }

    /// Active Markdown parser backend for this document.
    #[must_use]
    pub fn parse_backend(&self) -> crate::parse::ParseBackend {
        self.diagnostics.backend
    }

    /// Export the document as HTML (requires `static` feature).
    #[cfg(feature = "static")]
    pub fn to_html(&self) -> Result<String, crate::core::error::RenderError> {
        let html = crate::html::writer::blocks_to_html(&self.blocks)?;
        Ok(if self.gfm_tagfilter {
            crate::html::tagfilter::apply_gfm_tagfilter(&html)
        } else {
            html
        })
    }
}

#[cfg(feature = "static")]
/// Convert Markdown to HTML using the pulldown static export path.
pub fn markdown_to_html(input: &str) -> Result<String, crate::core::error::RenderError> {
    let doc = Document::parse(input, ParseProfile::GitHubPreview)
        .map_err(|e| crate::core::error::RenderError::new(e.to_string()))?;
    doc.to_html()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::block::BlockKind;
    #[test]
    fn hello_example_text_parses_nonempty() {
        const YOUR_TEXT: &str = r"
# Hello, World!
This is a markdown renderer <b>with inline HTML support!</b>
- You can mix and match markdown and HTML together
<hr>

```rust
App {
    state: MarkState::with_html_and_markdown(YOUR_TEXT)
}
```

## Note

> <b>Fun fact</b>: This is all built on top of existing iced widgets.
>
> No new widgets were made for this.
";
        let doc = Document::parse(YOUR_TEXT, ParseProfile::GitHubPreview).expect("parse");
        assert!(
            !doc.blocks().is_empty(),
            "expected blocks, got {}",
            doc.blocks().len()
        );
    }

    #[test]
    fn document_blocks_are_stable() {
        let doc =
            Document::parse("# Hi\n\nParagraph.", ParseProfile::GitHubPreview).expect("parse");
        let ids: Vec<_> = doc.blocks().iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), doc.blocks().len());
        assert!(doc.blocks().iter().any(|b| b.kind == BlockKind::Heading));
    }

    #[cfg(feature = "static")]
    #[test]
    fn raw_details_fixture_exports_children() {
        let source = include_str!("../../tests/fixtures/raw_details.md");
        let doc = Document::parse(source, ParseProfile::GitHubPreview).expect("parse");
        let html = doc.to_html().expect("html");
        assert!(
            html.contains("summary") || html.contains("Summary"),
            "html: {html}"
        );
    }

    #[cfg(feature = "static")]
    #[test]
    fn to_html_exports_headings() {
        let doc = Document::parse("# Hi", ParseProfile::GitHubPreview).expect("parse");
        let html = doc.to_html().expect("html");
        assert!(html.contains("<h1>"));
    }

    #[test]
    fn diagnostics_expose_pulldown_backend() {
        let doc = Document::parse("# Hi", ParseProfile::GitHubPreview).expect("parse");
        assert_eq!(doc.parse_backend(), crate::parse::ParseBackend::Pulldown);
        assert_eq!(doc.diagnostics().to_string(), "backend=pulldown");
    }

    #[cfg(feature = "static")]
    #[test]
    fn strip_unsupported_rejects_script_html() {
        use crate::options::RawHtmlPolicy;

        let mut options = ParseOptions::for_profile(ParseProfile::GitHubPreview);
        options.raw_html = RawHtmlPolicy::StripUnsupported;
        let doc = Document::parse_with_options(
            "<script>alert(1)</script>",
            ParseProfile::GitHubPreview,
            &options,
        )
        .expect("parse");
        assert!(
            doc.blocks().iter().any(|b| matches!(
                b.content,
                crate::core::block::BlockContent::Unsupported { .. }
                    | crate::core::block::BlockContent::Html(_)
            )),
            "script input should degrade safely into non-live content"
        );
        let html = doc.to_html().expect("html");
        assert!(!html.contains("<script"));
        assert!(html.contains("&lt;script>") || html.contains("unsupported"));
    }

    #[cfg(feature = "static")]
    #[test]
    fn explicit_gfm_tagfilter_setting_is_preserved_for_export() {
        let mut options = ParseOptions::for_profile(ParseProfile::GitHubPreview);
        options.gfm_tagfilter = false;
        let doc =
            Document::parse_with_options("<xmp>raw</xmp>", ParseProfile::GitHubPreview, &options)
                .expect("parse");
        let html = doc.to_html().expect("html");

        assert!(
            html.contains("<xmp>raw</xmp>"),
            "tagfilter unexpectedly applied: {html}"
        );
    }

    #[cfg(feature = "static")]
    #[test]
    fn gfm_wikilink_exports_via_pulldown() {
        let source = include_str!("../../tests/fixtures/gfm_wikilink.md");
        let doc = Document::parse(source, ParseProfile::GitHubPreview).expect("parse");
        assert_eq!(doc.parse_backend(), crate::parse::ParseBackend::Pulldown);
        let html = doc.to_html().expect("html");
        assert!(
            html.contains("WikiPage") || html.contains("wiki"),
            "wikilink html: {html}"
        );
    }
}
