//! Turning a [`Diagnostic`] into text a human can read.
//!
//! The rendered shape is:
//!
//! ```text
//! error[NOTO0400]: type mismatch
//!   --> examples/hello.noto:3:17
//!    |
//!  3 |     val n: Int = "text"
//!    |                  ^^^^^^ expected `Int`, found `String`
//!    |
//!    = help: use `"text".toInt()` to parse the string
//! ```

use crate::{Diagnostic, Label};
use noto_span::{SourceMap, Span};

/// Whether to colour the output with ANSI escapes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderStyle {
    /// No escape sequences; safe for files, tests and pipes.
    Plain,
    /// ANSI colours for an interactive terminal.
    Ansi,
}

struct Palette {
    reset: &'static str,
    bold: &'static str,
    severity: &'static str,
    gutter: &'static str,
    primary: &'static str,
    secondary: &'static str,
}

impl Palette {
    fn for_style(style: RenderStyle, severity: crate::Severity) -> Palette {
        match style {
            RenderStyle::Plain => Palette {
                reset: "",
                bold: "",
                severity: "",
                gutter: "",
                primary: "",
                secondary: "",
            },
            RenderStyle::Ansi => Palette {
                reset: "\u{1b}[0m",
                bold: "\u{1b}[1m",
                severity: match severity {
                    crate::Severity::Note => "\u{1b}[1;36m",
                    crate::Severity::Warning => "\u{1b}[1;33m",
                    crate::Severity::Error | crate::Severity::Fatal => "\u{1b}[1;31m",
                },
                gutter: "\u{1b}[1;34m",
                primary: "\u{1b}[1;31m",
                secondary: "\u{1b}[1;34m",
            },
        }
    }
}

/// Renders one diagnostic against the source it refers to.
pub fn render(diagnostic: &Diagnostic, map: &SourceMap, style: RenderStyle) -> String {
    let p = Palette::for_style(style, diagnostic.severity);
    let mut out = String::new();

    out.push_str(&format!(
        "{}{}[{}]{}: {}{}{}\n",
        p.severity,
        diagnostic.severity.label(),
        diagnostic.code,
        p.reset,
        p.bold,
        diagnostic.message,
        p.reset,
    ));

    let labels: Vec<&Label> = diagnostic.labels.iter().filter(|l| !l.span.is_dummy()).collect();
    let gutter_width = labels
        .iter()
        .filter_map(|label| line_of(map, label.span))
        .max()
        .map(|line| line.to_string().len())
        .unwrap_or(1);

    if let Some(first) = labels.iter().find(|l| l.primary).or_else(|| labels.first()) {
        if let Some(file) = map.file(first.span.file) {
            let pos = file.line_col(first.span.start);
            out.push_str(&format!(
                "{}{:width$}--> {}{}:{}:{}\n",
                p.gutter,
                "",
                p.reset,
                file.name(),
                pos.line,
                pos.column,
                width = gutter_width
            ));
        }
    }

    let mut rendered_any = false;
    for label in &labels {
        if let Some(snippet) = render_snippet(label, map, &p, gutter_width) {
            if !rendered_any {
                out.push_str(&format!("{}{:width$} |{}\n", p.gutter, "", p.reset, width = gutter_width));
            }
            out.push_str(&snippet);
            rendered_any = true;
        }
    }

    if rendered_any && (!diagnostic.helps.is_empty() || !diagnostic.notes.is_empty()) {
        out.push_str(&format!("{}{:width$} |{}\n", p.gutter, "", p.reset, width = gutter_width));
    }
    for help in &diagnostic.helps {
        out.push_str(&format!(
            "{}{:width$} = {}help: {}\n",
            p.gutter,
            "",
            p.reset,
            help,
            width = gutter_width
        ));
    }
    for note in &diagnostic.notes {
        out.push_str(&format!(
            "{}{:width$} = {}note: {}\n",
            p.gutter,
            "",
            p.reset,
            note,
            width = gutter_width
        ));
    }

    out
}

fn line_of(map: &SourceMap, span: Span) -> Option<u32> {
    Some(map.file(span.file)?.line_col(span.start).line)
}

fn render_snippet(
    label: &Label,
    map: &SourceMap,
    p: &Palette,
    gutter_width: usize,
) -> Option<String> {
    let file = map.file(label.span.file)?;
    let start = file.line_col(label.span.start);
    let text = file.line_text(start.line)?;

    let mut out = String::new();
    out.push_str(&format!(
        "{}{:>width$} |{} {}\n",
        p.gutter,
        start.line,
        p.reset,
        text,
        width = gutter_width
    ));

    // Underline runs to the end of the line for multi-line spans; carets are
    // measured in characters so they line up under wide source text.
    let end = file.line_col(label.span.end);
    let underline_len = if end.line == start.line {
        (end.column.saturating_sub(start.column)).max(1)
    } else {
        (text.chars().count() as u32 + 1).saturating_sub(start.column).max(1)
    };

    let caret = if label.primary { '^' } else { '-' };
    let colour = if label.primary { p.primary } else { p.secondary };
    let pad = " ".repeat(start.column.saturating_sub(1) as usize);
    let marks: String = std::iter::repeat(caret).take(underline_len as usize).collect();

    out.push_str(&format!(
        "{}{:width$} |{} {}{}{}{}{}\n",
        p.gutter,
        "",
        p.reset,
        pad,
        colour,
        marks,
        if label.message.is_empty() {
            String::new()
        } else {
            format!(" {}", label.message)
        },
        p.reset,
        width = gutter_width
    ));

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codes, Diagnostic};

    #[test]
    fn renders_a_pointed_error() {
        let mut map = SourceMap::new();
        let file = map.add("hello.noto", "fn main() {\n    val n: Int = \"text\"\n}\n");
        let span = Span::new(file, 29, 35);

        let d = Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch")
            .with_primary(span, "expected `Int`, found `String`")
            .with_help("use `\"text\".toInt()` to parse the string");

        let text = render(&d, &map, RenderStyle::Plain);
        let expected = "\
error[NOTO0400]: type mismatch
 --> hello.noto:2:18
  |
2 |     val n: Int = \"text\"
  |                  ^^^^^^ expected `Int`, found `String`
  |
  = help: use `\"text\".toInt()` to parse the string
";
        assert_eq!(text, expected);
    }

    #[test]
    fn renders_secondary_labels_with_dashes() {
        let mut map = SourceMap::new();
        let file = map.add("a.noto", "val x = 1\nx = 2\n");
        let d = Diagnostic::error(codes::REASSIGNED_VAL, "cannot reassign `x`")
            .with_primary(Span::new(file, 10, 11), "reassigned here")
            .with_secondary(Span::new(file, 4, 5), "declared with `val` here");

        let text = render(&d, &map, RenderStyle::Plain);
        assert!(text.contains("^ reassigned here"), "{text}");
        assert!(text.contains("- declared with `val` here"), "{text}");
    }

    #[test]
    fn a_diagnostic_without_spans_still_renders() {
        let map = SourceMap::new();
        let d = Diagnostic::error(codes::NO_MAIN, "no `main` function found");
        let text = render(&d, &map, RenderStyle::Plain);
        assert_eq!(text, "error[NOTO0002]: no `main` function found\n");
    }

    #[test]
    fn ansi_style_emits_escapes() {
        let mut map = SourceMap::new();
        let file = map.add("a.noto", "val x = 1\n");
        let d = Diagnostic::error(codes::TYPE_MISMATCH, "boom")
            .with_primary(Span::new(file, 4, 5), "here");
        let text = render(&d, &map, RenderStyle::Ansi);
        assert!(text.contains("\u{1b}[1;31m"));
    }
}
