# StriMD (headless)

Headless builds use `default-features = false` with `no_iced` plus `static` and/or `stream`. For the full headless stack (static + streaming + harness parity), enable all three: `features = ["no_iced", "static", "stream"]`.

## Static preview and HTML export

```rust,no_run
#[cfg(feature = "static")]
{
use strimd::{Document, ParseProfile};

let doc = Document::parse("# Title\n\nBody.", ParseProfile::GitHubPreview).unwrap();
let html = doc.to_html().unwrap();
let _ = html;
}
```

## LLM streaming

```rust,no_run
#[cfg(feature = "stream")]
{
use strimd::{StreamDocument, StreamOptions};

let mut doc = StreamDocument::new(StreamOptions::chat());
doc.append("Hello ");
doc.append("**world**.\n\n");
}
```

## Supported public features

| Feature | Purpose |
|---------|---------|
| `no_iced` | Disable the default iced backend |
| `static` | `Document::parse` and `Document::to_html()` |
| `stream` | `StreamDocument` incremental parsing |

See [API.md](API.md) for the full public API, the repository [README](../README.md) for iced `MarkWidget` usage, and [MIGRATION.md](MIGRATION.md) for the stack migration guide.
