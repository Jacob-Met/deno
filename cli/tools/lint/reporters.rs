// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashSet;

use deno_ast::SourceTextInfo;
use deno_ast::diagnostics::Diagnostic;
use deno_ast::diagnostics::DiagnosticLevel;
use deno_ast::diagnostics::DiagnosticLocation;
use deno_ast::diagnostics::DiagnosticSnippet;
use deno_ast::diagnostics::DiagnosticSnippetHighlight;
use deno_ast::diagnostics::DiagnosticSnippetHighlightStyle;
use deno_ast::diagnostics::DiagnosticSourcePos;
use deno_ast::diagnostics::DiagnosticSourceRange;
use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_lib::util::result::js_error_downcast_ref;
use deno_lint::diagnostic::LintDiagnostic;
use deno_runtime::colors;
use deno_runtime::fmt_errors::format_js_error;
use log::info;
use serde::Serialize;

use crate::args::LintReporterKind;

const JSON_SCHEMA_VERSION: u8 = 1;

pub fn create_reporter(kind: LintReporterKind) -> Box<dyn LintReporter + Send> {
  match kind {
    LintReporterKind::Pretty => Box::new(PrettyLintReporter::new()),
    LintReporterKind::Json => Box::new(JsonLintReporter::new()),
    LintReporterKind::Compact => Box::new(CompactLintReporter::new()),
  }
}

pub trait LintReporter {
  fn visit_diagnostic(&mut self, d: &LintDiagnostic);
  fn visit_error(&mut self, file_path: &str, err: &AnyError);
  fn close(&mut self, check_count: usize);
}

// Keep the diagnostic's real location, rule, and hint while showing only the
// source around its highlight. Long minified lines otherwise render in full
// once for every lint error, producing quadratic amounts of terminal output.
struct WindowedLintDiagnostic<'a> {
  original: &'a LintDiagnostic,
  source: SourceTextInfo,
  highlight_start: usize,
  highlight_end: usize,
  line_index: usize,
}

impl<'a> WindowedLintDiagnostic<'a> {
  fn new(original: &'a LintDiagnostic) -> Option<Self> {
    const MAX_SOURCE_CHARS: usize = 240;
    const CONTEXT_BEFORE: usize = 60;
    const CONTEXT_AFTER: usize = 100;
    const ELLIPSIS: &str = "…";

    let range = original.range.as_ref()?;
    let original_source = &range.text_info;
    let line_index = original_source.line_index(range.range.start);
    if original_source.line_index(range.range.end) != line_index {
      return None; // Leave multiline diagnostics unchanged.
    }
    let line = original_source.line_text(line_index);
    // Only scan as far as the threshold, then find the window from the
    // diagnostic offset. A megabyte-long minified line must not be rescanned
    // and allocated once for each of its hundreds of lint errors.
    line.chars().nth(MAX_SOURCE_CHARS)?;

    let start_offset =
      range.range.start - original_source.line_start(line_index);
    let end_offset = range.range.end - original_source.line_start(line_index);
    if !line.is_char_boundary(start_offset) {
      return None;
    }
    let first_byte = line[..start_offset]
      .char_indices()
      .rev()
      .nth(CONTEXT_BEFORE - 1)
      .map(|(offset, _)| offset)
      .unwrap_or(0);
    let last_byte = line[start_offset..]
      .char_indices()
      .nth(CONTEXT_AFTER)
      .map(|(offset, _)| start_offset + offset)
      .unwrap_or(line.len());

    let prefix = if first_byte > 0 { ELLIPSIS } else { "" };
    let suffix = if last_byte < line.len() { ELLIPSIS } else { "" };
    let snippet_line =
      format!("{prefix}{}{suffix}", &line[first_byte..last_byte]);
    // Retain the original line number in the excerpt's gutter, while the
    // location in the diagnostic itself uses the untouched source.
    let source = SourceTextInfo::from_string(format!(
      "{}{snippet_line}",
      "\n".repeat(line_index)
    ));
    let prefix_len = prefix.len();
    let highlight_start = prefix_len + start_offset - first_byte;
    let highlight_end =
      prefix_len + end_offset.min(last_byte).max(start_offset) - first_byte;

    Some(Self {
      original,
      source,
      highlight_start,
      highlight_end,
      line_index,
    })
  }
}

impl Diagnostic for WindowedLintDiagnostic<'_> {
  fn level(&self) -> DiagnosticLevel {
    self.original.level()
  }

  fn code(&self) -> Cow<'_, str> {
    self.original.code()
  }

  fn message(&self) -> Cow<'_, str> {
    self.original.message()
  }

  fn location(&self) -> DiagnosticLocation<'_> {
    self.original.location()
  }

  fn snippet(&self) -> Option<DiagnosticSnippet<'_>> {
    let line_start = self.source.line_start(self.line_index);
    Some(DiagnosticSnippet {
      source: Cow::Borrowed(&self.source),
      highlights: vec![DiagnosticSnippetHighlight {
        range: DiagnosticSourceRange {
          start: DiagnosticSourcePos::SourcePos(
            line_start + self.highlight_start,
          ),
          end: DiagnosticSourcePos::SourcePos(line_start + self.highlight_end),
        },
        style: DiagnosticSnippetHighlightStyle::Error,
        description: self
          .original
          .range
          .as_ref()
          .and_then(|range| range.description.as_deref())
          .map(Cow::Borrowed),
      }],
    })
  }

  fn hint(&self) -> Option<Cow<'_, str>> {
    self.original.hint()
  }

  fn snippet_fixed(&self) -> Option<DiagnosticSnippet<'_>> {
    self.original.snippet_fixed()
  }

  fn info(&self) -> Cow<'_, [Cow<'_, str>]> {
    self.original.info()
  }

  fn docs_url(&self) -> Option<Cow<'_, str>> {
    self.original.docs_url()
  }
}

struct PrettyLintReporter {
  lint_count: u32,
  fixable_diagnostics: u32,
  shortened_files: HashSet<String>,
}

impl PrettyLintReporter {
  fn new() -> PrettyLintReporter {
    PrettyLintReporter {
      lint_count: 0,
      fixable_diagnostics: 0,
      shortened_files: HashSet::new(),
    }
  }
}

impl LintReporter for PrettyLintReporter {
  fn visit_diagnostic(&mut self, d: &LintDiagnostic) {
    self.lint_count += 1;
    if !d.details.fixes.is_empty() {
      self.fixable_diagnostics += 1;
    }

    if let Some(window) = WindowedLintDiagnostic::new(d) {
      if self.shortened_files.insert(d.specifier.to_string()) {
        log::warn!(
          "Long source line shortened in diagnostics. If this is generated code, exclude it using `lint.exclude` in deno.json."
        );
      }
      log::error!("{}\n", window.display());
    } else {
      log::error!("{}\n", d.display());
    }
  }

  fn visit_error(&mut self, file_path: &str, err: &AnyError) {
    log::error!("Error linting: {file_path}");
    let text = match js_error_downcast_ref(err) {
      Some(js_error) => format_js_error(js_error, None),
      None => format!("{err:#}"),
    };
    for line in text.split('\n') {
      if line.is_empty() {
        log::error!("");
      } else {
        log::error!("    {}", line);
      }
    }
  }

  fn close(&mut self, check_count: usize) {
    let fixable_suffix = if self.fixable_diagnostics > 0 {
      colors::gray(format!(" ({} fixable via --fix)", self.fixable_diagnostics))
        .to_string()
    } else {
      "".to_string()
    };
    match self.lint_count {
      1 => info!("Found 1 problem{}", fixable_suffix),
      n if n > 1 => {
        info!("Found {} problems{}", self.lint_count, fixable_suffix)
      }
      _ => (),
    }

    match check_count {
      1 => info!("Checked 1 file"),
      n => info!("Checked {} files", n),
    }
  }
}

struct CompactLintReporter {
  lint_count: u32,
}

impl CompactLintReporter {
  fn new() -> CompactLintReporter {
    CompactLintReporter { lint_count: 0 }
  }
}

impl LintReporter for CompactLintReporter {
  fn visit_diagnostic(&mut self, d: &LintDiagnostic) {
    self.lint_count += 1;

    match &d.range {
      Some(range) => {
        let text_info = &range.text_info;
        let range = &range.range;
        let line_and_column = text_info.line_and_column_display(range.start);
        log::error!(
          "{}: line {}, col {} - {} ({})",
          d.specifier,
          line_and_column.line_number,
          line_and_column.column_number,
          d.message(),
          d.code(),
        )
      }
      None => {
        log::error!("{}: {} ({})", d.specifier, d.message(), d.code())
      }
    }
  }

  fn visit_error(&mut self, file_path: &str, err: &AnyError) {
    log::error!("Error linting: {file_path}");
    log::error!("   {err}");
  }

  fn close(&mut self, check_count: usize) {
    match self.lint_count {
      1 => info!("Found 1 problem"),
      n if n > 1 => info!("Found {} problems", self.lint_count),
      _ => (),
    }

    match check_count {
      1 => info!("Checked 1 file"),
      n => info!("Checked {} files", n),
    }
  }
}

// WARNING: Ensure doesn't change because it's used in the JSON output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonDiagnosticLintPosition {
  /// The 1-indexed line number.
  pub line: usize,
  /// The 0-indexed column index.
  pub col: usize,
  pub byte_pos: usize,
}

impl JsonDiagnosticLintPosition {
  pub fn new(byte_index: usize, loc: deno_ast::LineAndColumnIndex) -> Self {
    JsonDiagnosticLintPosition {
      line: loc.line_index + 1,
      col: loc.column_index,
      byte_pos: byte_index,
    }
  }
}

// WARNING: Ensure doesn't change because it's used in the JSON output
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct JsonLintDiagnosticRange {
  pub start: JsonDiagnosticLintPosition,
  pub end: JsonDiagnosticLintPosition,
}

// WARNING: Ensure doesn't change because it's used in the JSON output
#[derive(Clone, Serialize)]
struct JsonLintDiagnostic {
  pub filename: String,
  pub range: Option<JsonLintDiagnosticRange>,
  pub message: String,
  pub code: String,
  pub hint: Option<String>,
}

#[derive(Serialize)]
struct LintError {
  file_path: String,
  message: String,
}

#[derive(Serialize)]
struct JsonLintReporter {
  version: u8,
  diagnostics: Vec<JsonLintDiagnostic>,
  errors: Vec<LintError>,
  checked_files: Vec<String>,
}

impl JsonLintReporter {
  fn new() -> JsonLintReporter {
    JsonLintReporter {
      version: JSON_SCHEMA_VERSION,
      diagnostics: Vec::new(),
      errors: Vec::new(),
      checked_files: Vec::new(),
    }
  }
}

impl LintReporter for JsonLintReporter {
  fn visit_diagnostic(&mut self, d: &LintDiagnostic) {
    self.diagnostics.push(JsonLintDiagnostic {
      filename: d.specifier.to_string(),
      range: d.range.as_ref().map(|range| {
        let text_info = &range.text_info;
        let range = range.range;
        JsonLintDiagnosticRange {
          start: JsonDiagnosticLintPosition::new(
            range.start.as_byte_index(text_info.range().start),
            text_info.line_and_column_index(range.start),
          ),
          end: JsonDiagnosticLintPosition::new(
            range.end.as_byte_index(text_info.range().start),
            text_info.line_and_column_index(range.end),
          ),
        }
      }),
      message: d.message().to_string(),
      code: d.code().to_string(),
      hint: d.hint().map(|h| h.to_string()),
    });

    let file_path = d
      .specifier
      .to_file_path()
      .unwrap()
      .to_string_lossy()
      .into_owned();

    if !self.checked_files.contains(&file_path) {
      self.checked_files.push(file_path);
    }
  }

  fn visit_error(&mut self, file_path: &str, err: &AnyError) {
    self.errors.push(LintError {
      file_path: file_path.to_string(),
      message: err.to_string(),
    });

    if !self.checked_files.contains(&file_path.to_string()) {
      self.checked_files.push(file_path.to_string());
    }
  }

  fn close(&mut self, _check_count: usize) {
    sort_diagnostics(&mut self.diagnostics);
    self.checked_files.sort();
    let json = serde_json::to_string_pretty(&self);
    #[allow(clippy::print_stdout, reason = "reporter")]
    {
      println!("{}", json.unwrap());
    }
  }
}

fn sort_diagnostics(diagnostics: &mut [JsonLintDiagnostic]) {
  // Sort so that we guarantee a deterministic output which is useful for tests
  diagnostics.sort_by(|a, b| {
    use std::cmp::Ordering;
    let file_order = a.filename.cmp(&b.filename);
    match file_order {
      Ordering::Equal => match &a.range {
        Some(a_range) => match &b.range {
          Some(b_range) => {
            let line_order = a_range.start.line.cmp(&b_range.start.line);
            match line_order {
              Ordering::Equal => a_range.start.col.cmp(&b_range.start.col),
              _ => line_order,
            }
          }
          None => Ordering::Less,
        },
        None => match &b.range {
          Some(_) => Ordering::Greater,
          None => Ordering::Equal,
        },
      },
      _ => file_order,
    }
  });
}
