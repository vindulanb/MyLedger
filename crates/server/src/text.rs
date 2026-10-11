//! Getting text out of an uploaded file.
//!
//! What this can and cannot do, plainly:
//!
//! - **PDFs with a text layer** — read, via the pure-Rust `pdf-extract`. Most
//!   invoices, statements and emailed receipts are these.
//! - **Plain text, CSV, Markdown** — read directly.
//! - **Photographs and scans** — *not* read. There is no OCR in this build.
//!   Recognising text in an image needs Tesseract, which is a C library with
//!   no workable pure-Rust equivalent; installing it would put a non-Rust
//!   runtime dependency into an app whose whole premise is Rust on hardware
//!   the user owns. A scan therefore produces no text and the document is
//!   left for the user to fill in by hand.
//! - **Spreadsheets and Word files** — not read. A `.xlsx` is a zip of XML;
//!   anyone exporting one can export a CSV instead, which is read.
//!
//! Every refusal comes back as a sentence fit to show someone, because a
//! document that silently yields nothing looks like a bug. Nothing here ever
//! invents a value it could not read.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Most characters kept from one document.
///
/// Generous enough for any receipt or invoice, bounded so that a 20 MiB PDF
/// of scanned book pages cannot put megabytes of text in the database and
/// then in every API response that mentions it.
pub const MAX_TEXT_CHARS: usize = 20_000;

/// What reading a file produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextOutcome {
    /// Text was read. Never empty or whitespace-only.
    Text(String),
    /// No text, and why — a sentence to show the user.
    Nothing(String),
}

impl TextOutcome {
    pub fn text(&self) -> Option<&str> {
        match self {
            TextOutcome::Text(text) => Some(text),
            TextOutcome::Nothing(_) => None,
        }
    }

    pub fn note(&self) -> Option<&str> {
        match self {
            TextOutcome::Text(_) => None,
            TextOutcome::Nothing(note) => Some(note),
        }
    }
}

/// Reads whatever text a file has.
///
/// Blocking and CPU-bound, so callers run it on a blocking thread.
pub fn read_text(filename: &str, mime_type: Option<&str>, bytes: &[u8]) -> TextOutcome {
    let kind = classify(filename, mime_type);

    match kind {
        Kind::Pdf => read_pdf(bytes),
        Kind::PlainText => {
            // Lossy: a stray byte in one line should not cost the whole file.
            let text = String::from_utf8_lossy(bytes);
            finish(&text, "this file contains no text")
        }
        Kind::Image => TextOutcome::Nothing(
            "MyLedger does not read text from images — it has no OCR. Add the details by hand, \
             or upload a PDF with a text layer."
                .to_string(),
        ),
        Kind::Spreadsheet => TextOutcome::Nothing(
            "MyLedger does not read spreadsheets. Export the same data as CSV and it will be \
             read, or import it directly from the Import action."
                .to_string(),
        ),
        Kind::Unknown(description) => TextOutcome::Nothing(format!(
            "MyLedger cannot read text from {description}. It reads PDFs with a text layer, and \
             plain text or CSV files."
        )),
    }
}

/// Pulls the text layer out of a PDF.
///
/// Wrapped in `catch_unwind` on purpose. `pdf-extract` parses files produced
/// by every PDF writer ever shipped, and a malformed one can panic deep
/// inside it. A user's odd receipt should get an explanation, not take down
/// the request handling it.
fn read_pdf(bytes: &[u8]) -> TextOutcome {
    let result = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_from_mem(bytes)));

    match result {
        Ok(Ok(text)) => finish(
            &text,
            "this PDF has no text layer, so it is most likely a scan or a photograph. MyLedger \
             has no OCR, so the details have to be added by hand.",
        ),
        Ok(Err(error)) => TextOutcome::Nothing(format!(
            "this PDF could not be read: {}",
            first_line(&error.to_string())
        )),
        Err(_) => TextOutcome::Nothing(
            "this PDF could not be read — the file appears to be damaged or uses something \
             MyLedger's PDF reader does not understand."
                .to_string(),
        ),
    }
}

/// Tidies text and decides whether there is any.
///
/// Whitespace-only counts as nothing: a PDF of a photograph often yields a
/// handful of newlines, and calling that "text" would send the rule engine
/// off to match against emptiness and report a mystifying no-match.
fn finish(text: &str, nothing_note: &str) -> TextOutcome {
    let cleaned = tidy(text);
    if cleaned.is_empty() {
        return TextOutcome::Nothing(nothing_note.to_string());
    }
    TextOutcome::Text(cleaned)
}

/// Collapses the whitespace a PDF extractor leaves behind, and caps length.
///
/// Extracted PDF text is full of runs of spaces and blank lines from the
/// original layout. Collapsing them makes a rule's pattern writable by hand:
/// `total\s*([0-9.]+)` should not have to anticipate eleven spaces.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT_CHARS));
    let mut blank_run = 0usize;

    for line in text.lines() {
        // Internal runs of whitespace become one space.
        let mut collapsed = String::with_capacity(line.len());
        let mut in_space = false;
        for ch in line.chars() {
            if ch.is_whitespace() {
                in_space = true;
                continue;
            }
            if in_space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            in_space = false;
            collapsed.push(ch);
        }

        if collapsed.is_empty() {
            // At most one blank line in a row survives.
            blank_run += 1;
            if blank_run > 1 || out.is_empty() {
                continue;
            }
            out.push('\n');
            continue;
        }
        blank_run = 0;

        out.push_str(&collapsed);
        out.push('\n');

        if out.chars().count() >= MAX_TEXT_CHARS {
            break;
        }
    }

    // Truncated on a character boundary, not a byte one.
    if out.chars().count() > MAX_TEXT_CHARS {
        out = out.chars().take(MAX_TEXT_CHARS).collect();
    }
    out.trim().to_string()
}

enum Kind {
    Pdf,
    PlainText,
    Image,
    Spreadsheet,
    Unknown(String),
}

/// Decides what a file is, from its type and then its name.
///
/// The declared type is preferred because a browser knows better than a file
/// extension, but a missing or generic type falls back to the extension.
fn classify(filename: &str, mime_type: Option<&str>) -> Kind {
    let mime = mime_type.unwrap_or("").to_lowercase();
    match mime.as_str() {
        "application/pdf" => return Kind::Pdf,
        "text/csv" | "text/plain" | "text/markdown" | "application/csv" => {
            return Kind::PlainText;
        }
        _ => {}
    }
    if mime.starts_with("image/") {
        return Kind::Image;
    }
    if mime.contains("spreadsheet") || mime.contains("ms-excel") || mime.contains("wordprocessing")
    {
        return Kind::Spreadsheet;
    }

    let lowered = filename.to_lowercase();
    let extension = lowered.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    match extension {
        "pdf" => Kind::Pdf,
        "txt" | "csv" | "tsv" | "md" | "text" | "log" => Kind::PlainText,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp"
        | "avif" => Kind::Image,
        "xlsx" | "xls" | "ods" | "docx" | "doc" | "odt" => Kind::Spreadsheet,
        "" => Kind::Unknown("a file with no extension".to_string()),
        other => Kind::Unknown(format!("a .{other} file")),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_read_as_itself() {
        let outcome = read_text("notes.txt", Some("text/plain"), b"TOTAL 12.50");
        assert_eq!(outcome.text(), Some("TOTAL 12.50"));
        assert_eq!(outcome.note(), None);
    }

    #[test]
    fn a_csv_is_text_too() {
        let outcome = read_text("statement.csv", Some("text/csv"), b"Date,Amount\n2026-10-01,5\n");
        assert!(outcome.text().unwrap().contains("Date,Amount"));
    }

    #[test]
    fn the_extension_is_used_when_the_type_is_missing_or_generic() {
        assert!(read_text("notes.txt", None, b"hello").text().is_some());
        assert!(
            read_text("notes.txt", Some("application/octet-stream"), b"hello").text().is_some()
        );
    }

    #[test]
    fn an_image_is_refused_with_an_explanation_about_ocr() {
        // Not silently empty: the user is owed the reason.
        let outcome = read_text("receipt.jpg", Some("image/jpeg"), b"\xff\xd8\xff\xe0 jpeg bytes");
        assert!(outcome.text().is_none());
        let note = outcome.note().unwrap();
        assert!(note.contains("no OCR"), "{note}");
        assert!(note.contains("by hand"), "{note}");
    }

    #[test]
    fn every_common_image_extension_is_recognised() {
        for name in ["a.png", "a.jpg", "a.jpeg", "a.heic", "a.webp", "a.tiff"] {
            let outcome = read_text(name, None, b"not really an image");
            assert!(outcome.note().unwrap().contains("OCR"), "{name}");
        }
    }

    #[test]
    fn a_spreadsheet_is_refused_and_points_at_csv_import() {
        let outcome = read_text("book.xlsx", None, b"PK\x03\x04");
        let note = outcome.note().unwrap();
        assert!(note.contains("CSV"), "{note}");
    }

    #[test]
    fn an_unknown_type_names_what_it_was() {
        let outcome = read_text("archive.zip", None, b"PK\x03\x04");
        assert!(outcome.note().unwrap().contains(".zip"), "{:?}", outcome.note());

        let outcome = read_text("README", None, b"x");
        assert!(outcome.note().unwrap().contains("no extension"), "{:?}", outcome.note());
    }

    #[test]
    fn an_empty_text_file_counts_as_no_text() {
        let outcome = read_text("empty.txt", Some("text/plain"), b"   \n\n  \n");
        assert!(outcome.text().is_none());
        assert!(outcome.note().unwrap().contains("no text"));
    }

    #[test]
    fn a_damaged_pdf_is_explained_rather_than_crashing_the_request() {
        // `pdf-extract` can panic on a malformed file; this must come back as
        // a message either way.
        let outcome = read_text("broken.pdf", Some("application/pdf"), b"%PDF-1.4 then nonsense");
        assert!(outcome.text().is_none());
        assert!(outcome.note().unwrap().contains("could not be read"), "{:?}", outcome.note());
    }

    #[test]
    fn whitespace_runs_are_collapsed_so_patterns_are_writable() {
        // Extracted PDF text is full of layout spacing. A rule should not
        // have to anticipate how many spaces a till printed.
        let messy = "CORNER   GROCER\n\n\n\n   TOTAL:        62.14   \n\n";
        let outcome = read_text("r.txt", Some("text/plain"), messy.as_bytes());
        // Runs of spaces become one space; runs of blank lines become one
        // blank line, which keeps a receipt's section breaks readable.
        assert_eq!(outcome.text().unwrap(), "CORNER GROCER\n\nTOTAL: 62.14");
    }

    #[test]
    fn line_structure_survives_because_rules_rely_on_it() {
        let outcome =
            read_text("r.txt", Some("text/plain"), b"Line one\nLine two\nLine three");
        assert_eq!(outcome.text().unwrap().lines().count(), 3);
    }

    #[test]
    fn very_long_text_is_capped() {
        let huge = "x".repeat(MAX_TEXT_CHARS * 2);
        let outcome = read_text("big.txt", Some("text/plain"), huge.as_bytes());
        let text = outcome.text().unwrap();
        assert!(text.chars().count() <= MAX_TEXT_CHARS, "{} chars", text.chars().count());
    }

    #[test]
    fn a_cap_falls_on_a_character_boundary() {
        // Multi-byte characters must not be cut in half.
        let huge = "é".repeat(MAX_TEXT_CHARS * 2);
        let outcome = read_text("big.txt", Some("text/plain"), huge.as_bytes());
        assert!(outcome.text().unwrap().chars().count() <= MAX_TEXT_CHARS);
    }

    #[test]
    fn invalid_utf8_does_not_lose_the_whole_file() {
        let mut bytes = b"TOTAL 12.50 ".to_vec();
        bytes.push(0xff);
        let outcome = read_text("r.txt", Some("text/plain"), &bytes);
        assert!(outcome.text().unwrap().contains("TOTAL 12.50"));
    }
}
