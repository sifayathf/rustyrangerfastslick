// ================= src/preview.rs =================
// Cross-platform file preview engine.
//
// PreviewContent enum:
//   Text        → plain Paragraph
//   Highlighted → colored spans (syntax-highlighted code)
//   Image       → ImageWidget (Buffer half-block rendering)

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{borrow::Cow, fs, io::Read, path::PathBuf, process::Command, sync::Arc};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ImageFallbackInfo {
    pub path: PathBuf,
    pub dimensions: Option<(u32, u32)>,
    pub img: Option<std::sync::Arc<image::DynamicImage>>,
}

#[derive(Clone)]
pub enum PreviewContent {
    Text(String),
    Highlighted(Vec<Line<'static>>),
    /// Same as `Highlighted`, but each line has a line-number gutter baked
    /// in — rendering this with word-wrap on breaks the gutter alignment,
    /// so the UI must render it unwrapped (clip instead of wrap).
    // Shared because the preview cache is consulted every frame. A deep
    // Vec<Line> clone made large notebooks allocate/copy their entire parsed
    // document on every scroll tick.
    Code(Arc<Vec<Line<'static>>>),
    ImageFallback(ImageFallbackInfo),
}

fn normalize_preview_content(content: PreviewContent) -> PreviewContent {
    fn normalize_lines(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        for line in &mut lines {
            for span in &mut line.spans {
                span.content = Cow::Owned(span.content.as_ref().nfc().collect::<String>());
            }
        }
        lines
    }

    match content {
        PreviewContent::Text(text) => PreviewContent::Text(text.nfc().collect()),
        PreviewContent::Highlighted(lines) => PreviewContent::Highlighted(normalize_lines(lines)),
        PreviewContent::Code(lines) => {
            PreviewContent::Code(Arc::new(normalize_lines(lines.as_ref().clone())))
        }
        image @ PreviewContent::ImageFallback(_) => image,
    }
}

// ── Syntax-highlight color palette (Catppuccin Mocha) ────────────────────────
const SH_KW: Color = Color::Rgb(203, 166, 247); // mauve   — keywords
const SH_STR: Color = Color::Rgb(166, 227, 161); // green   — strings
const SH_CMT: Color = Color::Rgb(108, 112, 134); // overlay — comments
const SH_NUM: Color = Color::Rgb(250, 179, 135); // peach   — numbers
const SH_OP: Color = Color::Rgb(137, 180, 250); // blue    — operators
const SH_FG: Color = Color::Rgb(205, 214, 244); // text    — default
const SH_LN: Color = Color::Rgb(88, 91, 112); // surface — line numbers
const SH_TYPE: Color = Color::Rgb(249, 226, 175); // yellow  — types / builtins
const SH_FN: Color = Color::Rgb(116, 199, 236); // sky     — function names

// ── Image decode + transform cache ───────────────────────────────────────────

struct ImgCache {
    path: PathBuf,
    rotation: u32,
    flip_h: bool,
    img: Arc<image::DynamicImage>,
}

static IMG_CACHE: Lazy<Mutex<Option<ImgCache>>> = Lazy::new(|| Mutex::new(None));
static OFFICE_RENDER_FAILURES: Lazy<Mutex<std::collections::HashMap<PathBuf, std::time::Instant>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));
static VIDEO_RENDER_FAILURES: Lazy<Mutex<std::collections::HashMap<PathBuf, std::time::Instant>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));
static VISUAL_RENDER_PENDING: Lazy<Mutex<std::collections::HashSet<PathBuf>>> =
    Lazy::new(|| Mutex::new(std::collections::HashSet::new()));

struct PreviewCacheEntry {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    len: u64,
    office_full: bool,
    pdf_visual: bool,
    slide_idx: usize,
    content: PreviewContent,
}

static PREVIEW_CACHE: Lazy<Mutex<Option<PreviewCacheEntry>>> = Lazy::new(|| Mutex::new(None));

fn get_or_decode(path: &PathBuf, rotation: u32, flip_h: bool) -> Option<Arc<image::DynamicImage>> {
    {
        let g = IMG_CACHE.lock();
        if let Some(c) = g.as_ref() {
            if &c.path == path && c.rotation == rotation && c.flip_h == flip_h {
                return Some(Arc::clone(&c.img));
            }
        }
    }
    let raw = image::open(path).ok()?;
    let rotated = match rotation {
        90 => raw.rotate90(),
        180 => raw.rotate180(),
        270 => raw.rotate270(),
        _ => raw,
    };
    let final_img = if flip_h { rotated.fliph() } else { rotated };
    let arc = Arc::new(final_img);
    *IMG_CACHE.lock() = Some(ImgCache {
        path: path.clone(),
        rotation,
        flip_h,
        img: Arc::clone(&arc),
    });
    Some(arc)
}

// ── Directory listing cache ───────────────────────────────────────────────────

struct DirCache {
    path: PathBuf,
    entries: Vec<crate::state::DirEntryInfo>,
}

static DIR_CACHE: Lazy<Mutex<Option<DirCache>>> = Lazy::new(|| Mutex::new(None));

pub fn cached_list_dir(path: &PathBuf) -> Vec<crate::state::DirEntryInfo> {
    {
        let g = DIR_CACHE.lock();
        if let Some(c) = g.as_ref() {
            if &c.path == path {
                return c.entries.clone();
            }
        }
    }
    let entries = crate::state::list_dir(path).unwrap_or_default();
    *DIR_CACHE.lock() = Some(DirCache {
        path: path.clone(),
        entries: entries.clone(),
    });
    entries
}

/// Drop the directory-preview cache after a filesystem mutation.
/// Passing `None` clears it unconditionally; a path only clears a matching
/// cached directory.
pub fn invalidate_cached_dir(path: Option<&std::path::Path>) {
    let mut cache = DIR_CACHE.lock();
    let should_clear = match (cache.as_ref(), path) {
        (Some(entry), Some(path)) => entry.path == path,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if should_clear {
        *cache = None;
    }
}

// ── Public render function ────────────────────────────────────────────────────

pub fn render(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    office_mode: crate::state::OfficeRenderMode,
    pdf_mode: crate::state::PdfRenderMode,
    slide_idx: usize,
) -> PreviewContent {
    if p.is_dir() {
        return PreviewContent::Text(String::new());
    }

    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let metadata = fs::metadata(p).ok();
    let modified = metadata.as_ref().and_then(|value| value.modified().ok());
    let len = metadata.as_ref().map_or(0, |value| value.len());
    let office_full = office_mode == crate::state::OfficeRenderMode::Full;
    let pdf_visual = pdf_mode == crate::state::PdfRenderMode::Visual;
    let is_video = is_video_extension(&ext);
    let visual_requested = (office_full
        && matches!(
            ext.as_str(),
            "doc" | "docx" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
        ))
        || (pdf_visual && ext == "pdf")
        || is_video;
    let ready_visual_path = if office_full
        && matches!(
            ext.as_str(),
            "doc" | "docx" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
        ) {
        Some(get_office_cache_path(p, slide_idx))
    } else if pdf_visual && ext == "pdf" {
        Some(get_pdf_cache_path(p, slide_idx))
    } else if is_video {
        Some(get_video_cache_path(p))
    } else {
        None
    };
    let visual_ready = ready_visual_path.as_ref().is_some_and(|path| path.exists());
    if let Some(cache) = PREVIEW_CACHE.lock().as_ref() {
        if cache.path == *p
            && cache.modified == modified
            && cache.len == len
            && cache.office_full == office_full
            && cache.pdf_visual == pdf_visual
            && cache.slide_idx == slide_idx
        {
            let cached_ready_visual = match (&cache.content, ready_visual_path.as_ref()) {
                (PreviewContent::ImageFallback(info), Some(ready_path)) => info.path == *ready_path,
                _ => false,
            };
            if !visual_requested || !visual_ready || cached_ready_visual {
                return cache.content.clone();
            }
        }
    }

    let content = normalize_preview_content(match ext.as_str() {
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" | "tiff" | "tif" | "ico" => {
            render_image(p, rotation, flip_h)
        }
        extension if is_video_extension(extension) => render_video(p),
        "mp3" | "flac" | "wav" | "ogg" | "aac" | "m4a" | "opus" | "wma" => render_audio(p),
        "docx" | "doc" => render_docx(p, rotation, flip_h, office_mode),
        "xlsx" | "xls" | "ods" => render_excel(p, rotation, flip_h, office_mode),
        "pptx" | "ppt" | "odp" => render_pptx(p, rotation, flip_h, office_mode, slide_idx),
        "pdf" => render_pdf(p, pdf_mode, slide_idx),
        "rtf" => render_rtf(p),
        "csv" | "tsv" => render_csv(p),
        "ipynb" => render_notebook(p),
        "zip" | "7z" | "tar" | "gz" | "bz2" | "xz" | "rar" | "tgz" => render_archive(p),
        _ => text_preview(p),
    });
    *PREVIEW_CACHE.lock() = Some(PreviewCacheEntry {
        path: p.clone(),
        modified,
        len,
        office_full,
        pdf_visual,
        slide_idx,
        content: content.clone(),
    });
    content
}

pub fn is_visual_preview(
    path: Option<&PathBuf>,
    office_mode: crate::state::OfficeRenderMode,
    pdf_mode: crate::state::PdfRenderMode,
) -> bool {
    let ext = path
        .and_then(|value| value.extension())
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" | "tiff" | "tif" | "ico"
    ) || (is_video_extension(&ext) && path.is_some_and(|path| get_video_cache_path(path).exists()))
        || (matches!(
            ext.as_str(),
            "doc" | "docx" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp"
        ) && office_mode == crate::state::OfficeRenderMode::Full
            && path.is_some_and(|path| {
                get_office_cache_path(path, 0).exists()
                    || IMG_CACHE
                        .lock()
                        .as_ref()
                        .is_some_and(|entry| entry.path == *path)
            }))
        || (ext == "pdf" && pdf_mode == crate::state::PdfRenderMode::Visual)
}

fn is_video_extension(ext: &str) -> bool {
    matches!(
        ext,
        "mp4"
            | "m4v"
            | "mkv"
            | "avi"
            | "mov"
            | "webm"
            | "flv"
            | "wmv"
            | "mpg"
            | "mpeg"
            | "mts"
            | "m2ts"
            | "3gp"
            | "vob"
            | "ogv"
    )
}

pub fn presentation_slide_count(path: &PathBuf) -> Option<usize> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("odp") {
        let file = fs::File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let mut content = String::new();
        archive
            .by_name("content.xml")
            .ok()?
            .read_to_string(&mut content)
            .ok()?;
        let count = content.matches("<draw:page ").count();
        return (count > 0).then_some(count);
    }
    if !extension.eq_ignore_ascii_case("pptx") {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let count = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_string())
        })
        .filter(|name| {
            let Some(stem) = name.strip_prefix("ppt/slides/slide") else {
                return false;
            };
            stem.strip_suffix(".xml").is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .count();
    (count > 0).then_some(count)
}

pub fn pdf_page_count(path: &PathBuf) -> Option<usize> {
    let count = lopdf::Document::load(path).ok()?.get_pages().len();
    (count > 0).then_some(count)
}

// ── Metadata header ───────────────────────────────────────────────────────────

pub fn meta_header(_p: &PathBuf) -> Vec<Line<'static>> {
    Vec::new()
}

/// Truncate to at most `max_chars` **characters** at a valid UTF-8 boundary.
/// Byte-length-based slicing (`&s[..s.len().min(N)]`) panics the moment the
/// string contains any multi-byte character within the first N bytes — this
/// doesn't.
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    match s.grapheme_indices(true).nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Runs a third-party parser call and converts an internal panic into `None`
/// instead of taking down the whole app. Binary-format parsers (PDF, XLSX,
/// ZIP, audio tags) are routinely handed malformed or password-protected
/// files, and libraries like `pdf-extract` are known to panic internally
/// (not return `Err`) on some of them — normal `Result` handling can't catch
/// that. We also swap in a silent panic hook for the duration of the call so
/// the default handler doesn't dump a backtrace to stderr, which corrupts
/// the alternate-screen TUI display even when we recover from it.
fn safe_parse<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(f);
    std::panic::set_hook(prev_hook);
    result.ok()
}

pub fn human_size(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    for &u in UNITS {
        if v < 1024.0 {
            return format!("{:.1} {}", v, u);
        }
        v /= 1024.0;
    }
    format!("{:.1} TB", v * 1024.0)
}

fn highlight_document_line(line: &str) -> Line<'static> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Line::from("");
    }

    // 1. Check for headings:
    // If the line is relatively short, doesn't end with a period/comma/colon, and has letters.
    // Also, if it has a high ratio of uppercase characters or is a section title.
    let char_count = trimmed.chars().count().max(1);
    let is_likely_heading = char_count < 50
        && !trimmed.ends_with('.')
        && !trimmed.ends_with(',')
        && !trimmed.contains(':')
        && trimmed.chars().any(|c| c.is_alphabetic())
        && (trimmed.chars().filter(|c| c.is_uppercase()).count() as f32 / char_count as f32 > 0.2
            || char_count < 25);

    if is_likely_heading {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // 2. Check for list items / bullets
    if trimmed.starts_with('•')
        || trimmed.starts_with('-')
        || trimmed.starts_with('*')
        || (trimmed.chars().next().map_or(false, |c| c.is_numeric()) && trimmed.contains(". "))
    {
        // Find the index of the bullet/number separator
        let sep_idx =
            if trimmed.starts_with('•') || trimmed.starts_with('-') || trimmed.starts_with('*') {
                trimmed.chars().next().map(|c| c.len_utf8()).unwrap_or(1)
            } else {
                trimmed.find(". ").map(|i| i + 2).unwrap_or(0)
            };

        // Guard against sep_idx landing outside the string or mid-character
        // (defensive — the branches above should already produce a valid
        // boundary, but a panic here would take down the whole app).
        if sep_idx == 0 || sep_idx > trimmed.len() || !trimmed.is_char_boundary(sep_idx) {
            return Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Rgb(205, 214, 244)),
            ));
        }

        let bullet = &trimmed[..sep_idx];
        let content = &trimmed[sep_idx..];

        return Line::from(vec![
            Span::styled(
                format!("  {} ", bullet),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                content.to_string(),
                Style::default().fg(Color::Rgb(205, 214, 244)),
            ),
        ]);
    }

    // 3. Normal paragraph text - check for emails/websites to highlight
    if trimmed.contains('@')
        || trimmed.contains("http://")
        || trimmed.contains("https://")
        || trimmed.contains("www.")
    {
        let mut spans = Vec::new();
        for word in line.split_whitespace() {
            if word.contains('@') || word.contains("://") || word.starts_with("www.") {
                spans.push(Span::styled(
                    format!("{} ", word),
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED),
                ));
            } else {
                spans.push(Span::styled(
                    format!("{} ", word),
                    Style::default().fg(Color::Rgb(205, 214, 244)),
                ));
            }
        }
        return Line::from(spans);
    }

    // Default: soft white paragraph text
    Line::from(Span::styled(
        line.to_string(),
        Style::default().fg(Color::Rgb(205, 214, 244)),
    ))
}

// ── Syntax highlighting ───────────────────────────────────────────────────────

fn highlight_code(text: &str, ext: &str) -> Vec<Line<'static>> {
    highlight_code_with_limit(text, ext, usize::MAX)
}

/// Syntax-highlight the live editor buffer without flattening its line
/// structure. Both editor and read-only previews retain the whole file; the UI
/// renders only the visible viewport.
pub fn highlight_editor_buffer(buffer: &[String], ext: &str) -> Vec<Line<'static>> {
    let display_text = buffer
        .iter()
        .map(|line| expand_editor_tabs(line))
        .collect::<Vec<_>>()
        .join("\n");
    highlight_code_with_limit(&display_text, ext, usize::MAX)
}

/// Expand tabs for display only. The underlying edit buffer keeps the exact
/// characters from disk, while the preview uses stable four-column tab stops.
pub fn expand_editor_tabs(line: &str) -> String {
    let mut result = String::new();
    let mut column = 0usize;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = 4 - (column % 4);
            result.push_str(&" ".repeat(spaces));
            column += spaces;
        } else {
            result.push(ch);
            column += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    result
}

fn highlight_code_with_limit(text: &str, ext: &str, max_lines: usize) -> Vec<Line<'static>> {
    let keywords = language_keywords(ext);
    let types_bi = language_types(ext);
    let cmt_line = line_comment_prefix(ext);
    let cmt_block = block_comment_chars(ext); // (open, close)

    let mut lines: Vec<Line<'static>> = Vec::new();
    let rendered_line_count = text.split('\n').take(max_lines).count().max(1);
    let gutter_digits = rendered_line_count.to_string().len();

    // We need cross-line block-comment tracking
    let mut in_block_cmt = false;

    for (ln_idx, raw_line) in text.split('\n').enumerate().take(max_lines) {
        let mut spans: Vec<Span<'static>> = Vec::new();

        // Line number gutter
        spans.push(Span::styled(
            format!("{:>width$}│ ", ln_idx + 1, width = gutter_digits),
            Style::default().fg(SH_LN),
        ));

        let chars: Vec<char> = raw_line.chars().collect();
        let n = chars.len();
        let mut i = 0;

        while i < n {
            // Inside a block comment — look for end marker
            if in_block_cmt {
                if let Some((_, close)) = cmt_block {
                    let remaining: String = chars[i..].iter().collect();
                    if let Some(pos) = remaining.find(close) {
                        let s: String = chars[i..i + pos + close.len()].iter().collect();
                        spans.push(Span::styled(s, Style::default().fg(SH_CMT)));
                        i += pos + close.len();
                        in_block_cmt = false;
                        continue;
                    }
                }
                // Whole rest of line is comment
                let rest: String = chars[i..].iter().collect();
                spans.push(Span::styled(rest, Style::default().fg(SH_CMT)));
                break;
            }

            // Block comment open?
            if let Some((open, _)) = cmt_block {
                let remaining: String = chars[i..].iter().collect();
                if remaining.starts_with(open) {
                    let close = cmt_block.unwrap().1;
                    let after_open = &remaining[open.len()..];
                    if let Some(end) = after_open.find(close) {
                        // whole block on this line
                        let len = open.len() + end + close.len();
                        let s: String = chars[i..i + len].iter().collect();
                        spans.push(Span::styled(s, Style::default().fg(SH_CMT)));
                        i += len;
                        continue;
                    } else {
                        // block continues to next line
                        let rest: String = chars[i..].iter().collect();
                        spans.push(Span::styled(rest, Style::default().fg(SH_CMT)));
                        in_block_cmt = true;
                        break;
                    }
                }
            }

            // Line comment?
            if !cmt_line.is_empty() {
                let remaining: String = chars[i..].iter().collect();
                if remaining.starts_with(cmt_line) {
                    spans.push(Span::styled(remaining, Style::default().fg(SH_CMT)));
                    break;
                }
            }

            // String literals
            if chars[i] == '"' || chars[i] == '`' || (chars[i] == '\'' && ext != "rs")
            // skip lifetime 'a in Rust
            {
                let quote = chars[i];
                let start = i;
                i += 1;
                while i < n {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    } // escape
                    if chars[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                spans.push(Span::styled(s, Style::default().fg(SH_STR)));
                continue;
            }

            // Numbers (hex, float, int)
            if chars[i].is_ascii_digit() {
                let start = i;
                // hex prefix
                if chars[i] == '0' && i + 1 < n && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                    i += 2;
                    while i < n && (chars[i].is_ascii_hexdigit() || chars[i] == '_') {
                        i += 1;
                    }
                } else {
                    while i < n
                        && (chars[i].is_ascii_digit()
                            || chars[i] == '.'
                            || chars[i] == '_'
                            || chars[i] == 'e'
                            || chars[i] == 'E')
                    {
                        i += 1;
                    }
                    // type suffix like u32, f64
                    while i < n && (chars[i].is_alphabetic() || chars[i] == '_') {
                        i += 1;
                    }
                }
                let s: String = chars[start..i].iter().collect();
                spans.push(Span::styled(s, Style::default().fg(SH_NUM)));
                continue;
            }

            // Identifiers / keywords / types
            if chars[i].is_alphabetic() || chars[i] == '_' {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();

                // Check next char: if '(' it's a function call
                let next_non_ws = chars[i..].iter().find(|&&c| c != ' ');
                let is_fn_call = next_non_ws == Some(&'(');

                let color = if keywords.contains(&word.as_str()) {
                    SH_KW
                } else if types_bi.contains(&word.as_str()) {
                    SH_TYPE
                } else if is_fn_call {
                    SH_FN
                } else {
                    SH_FG
                };
                spans.push(Span::styled(word, Style::default().fg(color)));
                continue;
            }

            // Operators
            let ch = chars[i];
            let s = ch.to_string();
            if "+-*/=<>!&|^~%@".contains(ch) {
                spans.push(Span::styled(s, Style::default().fg(SH_OP)));
            } else {
                spans.push(Span::styled(s, Style::default().fg(SH_FG)));
            }
            i += 1;
        }

        lines.push(Line::from(spans));
    }
    lines
}

fn language_keywords(ext: &str) -> std::collections::HashSet<&'static str> {
    let kws: &[&str] = match ext {
        "rs" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while", "box",
        ],
        "py" => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in",
            "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
            "with", "yield",
        ],
        "js" | "ts" | "jsx" | "tsx" => &[
            "abstract",
            "as",
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "enum",
            "export",
            "extends",
            "finally",
            "for",
            "from",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "of",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "try",
            "typeof",
            "var",
            "void",
            "while",
            "with",
            "yield",
            "type",
            "declare",
            "readonly",
            "implements",
            "namespace",
            "module",
            "satisfies",
            "override",
        ],
        "go" => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
        ],
        "java" | "kt" | "kts" => &[
            "abstract",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "else",
            "enum",
            "extends",
            "final",
            "finally",
            "for",
            "goto",
            "if",
            "implements",
            "import",
            "instanceof",
            "interface",
            "native",
            "new",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "static",
            "strictfp",
            "super",
            "switch",
            "synchronized",
            "this",
            "throw",
            "throws",
            "transient",
            "try",
            "var",
            "void",
            "volatile",
            "while",
            // Kotlin extras
            "data",
            "fun",
            "in",
            "is",
            "object",
            "open",
            "override",
            "sealed",
            "when",
            "companion",
            "lateinit",
            "inline",
            "reified",
            "suspend",
            "coroutine",
        ],
        "c" | "cpp" | "h" | "hpp" | "cc" => &[
            "auto",
            "break",
            "case",
            "char",
            "const",
            "continue",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extern",
            "float",
            "for",
            "goto",
            "if",
            "inline",
            "int",
            "long",
            "register",
            "return",
            "short",
            "signed",
            "sizeof",
            "static",
            "struct",
            "switch",
            "typedef",
            "union",
            "unsigned",
            "void",
            "volatile",
            "while",
            "class",
            "delete",
            "friend",
            "new",
            "operator",
            "private",
            "protected",
            "public",
            "template",
            "this",
            "throw",
            "try",
            "catch",
            "virtual",
            "override",
            "final",
            "nullptr",
            "constexpr",
            "decltype",
            "auto",
        ],
        "cs" => &[
            "abstract",
            "as",
            "base",
            "bool",
            "break",
            "byte",
            "case",
            "catch",
            "char",
            "checked",
            "class",
            "const",
            "continue",
            "decimal",
            "default",
            "delegate",
            "do",
            "double",
            "else",
            "enum",
            "event",
            "explicit",
            "extern",
            "false",
            "finally",
            "fixed",
            "float",
            "for",
            "foreach",
            "goto",
            "if",
            "implicit",
            "in",
            "int",
            "interface",
            "internal",
            "is",
            "lock",
            "long",
            "namespace",
            "new",
            "null",
            "object",
            "operator",
            "out",
            "override",
            "params",
            "private",
            "protected",
            "public",
            "readonly",
            "ref",
            "return",
            "sbyte",
            "sealed",
            "short",
            "sizeof",
            "stackalloc",
            "static",
            "string",
            "struct",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "typeof",
            "uint",
            "ulong",
            "unchecked",
            "unsafe",
            "ushort",
            "using",
            "virtual",
            "void",
            "volatile",
            "while",
            "async",
            "await",
            "dynamic",
            "var",
            "record",
            "init",
            "with",
        ],
        "rb" => &[
            "__FILE__",
            "__LINE__",
            "__ENCODING__",
            "BEGIN",
            "END",
            "alias",
            "and",
            "begin",
            "break",
            "case",
            "class",
            "def",
            "defined?",
            "do",
            "else",
            "elsif",
            "end",
            "ensure",
            "false",
            "for",
            "if",
            "in",
            "module",
            "next",
            "nil",
            "not",
            "or",
            "redo",
            "rescue",
            "retry",
            "return",
            "self",
            "super",
            "then",
            "true",
            "undef",
            "unless",
            "until",
            "when",
            "while",
            "yield",
        ],
        "php" => &[
            "abstract",
            "and",
            "array",
            "as",
            "break",
            "callable",
            "case",
            "catch",
            "class",
            "clone",
            "const",
            "continue",
            "declare",
            "default",
            "do",
            "echo",
            "else",
            "elseif",
            "empty",
            "enddeclare",
            "endfor",
            "endforeach",
            "endif",
            "endswitch",
            "endwhile",
            "eval",
            "exit",
            "extends",
            "final",
            "finally",
            "fn",
            "for",
            "foreach",
            "function",
            "global",
            "goto",
            "if",
            "implements",
            "include",
            "include_once",
            "instanceof",
            "insteadof",
            "interface",
            "isset",
            "list",
            "match",
            "namespace",
            "new",
            "null",
            "or",
            "print",
            "private",
            "protected",
            "public",
            "readonly",
            "require",
            "require_once",
            "return",
            "static",
            "switch",
            "throw",
            "trait",
            "try",
            "true",
            "unset",
            "use",
            "var",
            "while",
            "xor",
            "yield",
            "false",
        ],
        "swift" => &[
            "associatedtype",
            "class",
            "deinit",
            "enum",
            "extension",
            "fileprivate",
            "func",
            "import",
            "init",
            "inout",
            "internal",
            "let",
            "open",
            "operator",
            "private",
            "precedencegroup",
            "protocol",
            "public",
            "rethrows",
            "static",
            "struct",
            "subscript",
            "typealias",
            "var",
            "break",
            "case",
            "continue",
            "default",
            "defer",
            "do",
            "else",
            "fallthrough",
            "for",
            "guard",
            "if",
            "in",
            "repeat",
            "return",
            "throw",
            "switch",
            "where",
            "while",
            "as",
            "catch",
            "false",
            "is",
            "nil",
            "rethrows",
            "self",
            "super",
            "throw",
            "throws",
            "true",
            "try",
            "_",
        ],
        "lua" => &[
            "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto",
            "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until",
            "while",
        ],
        "sh" | "bash" | "zsh" => &[
            "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac",
            "function", "return", "local", "export", "echo", "read", "exit", "in", "source",
            "alias", "unset", "declare",
        ],
        "sql" => &[
            "select",
            "from",
            "where",
            "join",
            "left",
            "right",
            "inner",
            "outer",
            "on",
            "group",
            "order",
            "by",
            "having",
            "limit",
            "offset",
            "insert",
            "into",
            "values",
            "update",
            "set",
            "delete",
            "create",
            "table",
            "index",
            "view",
            "drop",
            "alter",
            "add",
            "column",
            "primary",
            "key",
            "foreign",
            "references",
            "unique",
            "not",
            "null",
            "default",
            "and",
            "or",
            "in",
            "like",
            "between",
            "exists",
            "union",
            "all",
            "distinct",
            "as",
            "with",
            "case",
            "when",
            "then",
            "else",
            "end",
            "count",
            "sum",
            "avg",
            "max",
            "min",
            "asc",
            "desc",
        ],
        "html" | "htm" => &[
            "doctype", "html", "head", "body", "div", "span", "p", "a", "img", "input", "button",
            "form", "table", "tr", "td", "th", "thead", "tbody", "ul", "ol", "li", "h1", "h2",
            "h3", "h4", "h5", "h6", "header", "footer", "nav", "main", "section", "article",
            "aside", "script", "style", "link", "meta", "title", "br", "hr", "strong", "em",
            "code", "pre",
        ],
        "css" | "scss" => &[
            "important",
            "media",
            "keyframes",
            "import",
            "charset",
            "font-face",
            "supports",
            "from",
            "to",
            "px",
            "em",
            "rem",
            "vh",
            "vw",
            "auto",
            "none",
            "inherit",
            "initial",
            "unset",
            "flex",
            "grid",
            "block",
            "inline",
            "absolute",
            "relative",
            "fixed",
            "sticky",
        ],
        _ => &[],
    };
    kws.iter().cloned().collect()
}

fn language_types(ext: &str) -> std::collections::HashSet<&'static str> {
    let types: &[&str] = match ext {
        "rs" => &[
            "String", "Vec", "HashMap", "HashSet", "Option", "Result", "Box", "Arc", "Rc", "Cell",
            "RefCell", "Mutex", "RwLock", "PathBuf", "Path", "OsStr", "OsString", "i8", "i16",
            "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
            "f64", "bool", "char", "str", "()", "Some", "None", "Ok", "Err", "true", "false",
        ],
        "py" => &[
            "int",
            "str",
            "float",
            "bool",
            "list",
            "dict",
            "set",
            "tuple",
            "bytes",
            "bytearray",
            "None",
            "True",
            "False",
            "type",
            "object",
            "super",
            "print",
            "len",
            "range",
            "enumerate",
            "zip",
            "map",
            "filter",
            "sorted",
            "reversed",
            "isinstance",
            "issubclass",
            "hasattr",
            "getattr",
            "setattr",
            "delattr",
            "TypeError",
            "ValueError",
            "KeyError",
            "IndexError",
            "AttributeError",
            "Exception",
            "RuntimeError",
            "StopIteration",
            "self",
            "cls",
        ],
        "ts" | "js" | "tsx" | "jsx" => &[
            "string",
            "number",
            "boolean",
            "null",
            "undefined",
            "never",
            "any",
            "unknown",
            "void",
            "object",
            "symbol",
            "bigint",
            "Array",
            "Promise",
            "Record",
            "Partial",
            "Required",
            "Readonly",
            "Pick",
            "Omit",
            "true",
            "false",
            "null",
        ],
        "go" => &[
            "bool",
            "byte",
            "complex64",
            "complex128",
            "error",
            "float32",
            "float64",
            "int",
            "int8",
            "int16",
            "int32",
            "int64",
            "rune",
            "string",
            "uint",
            "uint8",
            "uint16",
            "uint32",
            "uint64",
            "uintptr",
            "true",
            "false",
            "nil",
            "iota",
            "append",
            "cap",
            "close",
            "copy",
            "delete",
            "len",
            "make",
            "new",
            "panic",
            "print",
            "println",
            "recover",
        ],
        _ => &[],
    };
    types.iter().cloned().collect()
}

fn line_comment_prefix(ext: &str) -> &'static str {
    match ext {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "kt" | "kts" | "cs" | "swift"
        | "php" | "c" | "cpp" | "h" | "hpp" | "cc" => "//",
        "py" | "sh" | "bash" | "zsh" | "rb" | "toml" | "yaml" | "yml" | "dockerfile"
        | "gitignore" => "#",
        "lua" => "--",
        "sql" => "--",
        "html" | "htm" => "<!--",
        "css" | "scss" => "/*", // handled as block comment
        _ => "",
    }
}

fn block_comment_chars(ext: &str) -> Option<(&'static str, &'static str)> {
    match ext {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "kt" | "kts" | "cs" | "swift"
        | "c" | "cpp" | "h" | "hpp" | "cc" | "css" | "scss" => Some(("/*", "*/")),
        "html" | "htm" => Some(("<!--", "-->")),
        _ => None,
    }
}

// ── Image ─────────────────────────────────────────────────────────────────────

fn render_image(p: &PathBuf, rotation: u32, flip_h: bool) -> PreviewContent {
    let img = get_or_decode(p, rotation, flip_h);
    let dimensions = img.as_ref().map(|i| (i.width(), i.height()));
    PreviewContent::ImageFallback(ImageFallbackInfo {
        path: p.clone(),
        dimensions,
        img,
    })
}

// ── Video ─────────────────────────────────────────────────────────────────────

fn get_video_cache_path(p: &PathBuf) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(p.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(p) {
        hasher.update(meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified() {
            if let Ok(epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
                hasher.update(epoch.as_nanos().to_le_bytes());
            }
        }
    }
    let hash = hex::encode(hasher.finalize());
    std::env::temp_dir().join(format!("rr_video_{}.png", hash))
}

fn ffmpeg_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("ffmpeg"),
        PathBuf::from("C:\\ProgramData\\chocolatey\\bin\\ffmpeg.exe"),
        PathBuf::from("C:\\Program Files\\ffmpeg\\bin\\ffmpeg.exe"),
        PathBuf::from("C:\\ffmpeg\\bin\\ffmpeg.exe"),
    ];
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let winget = PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WinGet")
            .join("Links")
            .join("ffmpeg.exe");
        candidates.push(winget);
    }
    candidates
}

fn generate_video_thumbnail(source: &PathBuf, target: &PathBuf) -> bool {
    for ffmpeg in ffmpeg_candidates() {
        let status = Command::new(ffmpeg)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                "0.5",
                "-i",
            ])
            .arg(source)
            .args(["-an", "-frames:v", "1", "-vf", "scale='min(1280,iw)':-2"])
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) && target.exists() {
            return true;
        }
    }
    false
}

fn render_video(p: &PathBuf) -> PreviewContent {
    let cache_path = get_video_cache_path(p);
    if cache_path.exists() {
        return render_image(&cache_path, 0, false);
    }

    let failed_recently = VIDEO_RENDER_FAILURES
        .lock()
        .get(&cache_path)
        .is_some_and(|failed_at| failed_at.elapsed() < std::time::Duration::from_secs(30));
    let mut pending = false;
    if !failed_recently {
        let mut renders = VISUAL_RENDER_PENDING.lock();
        if renders.insert(cache_path.clone()) {
            pending = true;
            let source = p.clone();
            std::thread::spawn(move || {
                if generate_video_thumbnail(&source, &cache_path) {
                    VIDEO_RENDER_FAILURES.lock().remove(&cache_path);
                } else {
                    VIDEO_RENDER_FAILURES
                        .lock()
                        .insert(cache_path.clone(), std::time::Instant::now());
                }
                VISUAL_RENDER_PENDING.lock().remove(&cache_path);
                *PREVIEW_CACHE.lock() = None;
            });
        } else {
            pending = true;
        }
    }

    let mut lines = meta_header(p);
    lines.push(Line::from(Span::styled(
        "🎬  Video",
        Style::default().fg(SH_FN).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        if pending {
            "Thumbnail is decoding in the background — navigation stays responsive"
        } else {
            "Thumbnail backend unavailable; install FFmpeg or open in the default player"
        },
        Style::default().fg(SH_CMT),
    )));
    PreviewContent::Highlighted(lines)
}

// ── Audio ─────────────────────────────────────────────────────────────────────

fn render_audio(p: &PathBuf) -> PreviewContent {
    use lofty::prelude::*;
    use lofty::probe::Probe;

    let mut lines = meta_header(p);
    let name = p
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    match safe_parse(move || Probe::open(p).and_then(|pr| pr.read())) {
        Some(Ok(tagged_file)) => {
            let props = tagged_file.properties();
            let duration = props.duration();
            let mins = duration.as_secs() / 60;
            let secs = duration.as_secs() % 60;
            let bitrate = props
                .audio_bitrate()
                .map(|b| format!("{} kbps", b))
                .unwrap_or_else(|| "?".into());
            let sample_rate = props
                .sample_rate()
                .map(|s| format!("{} Hz", s))
                .unwrap_or_else(|| "?".into());
            let channels = props
                .channels()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into());

            let tag = tagged_file
                .primary_tag()
                .or_else(|| tagged_file.first_tag());
            let (title, artist, album, year, genre) = if let Some(t) = tag {
                (
                    t.title()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| name.clone()),
                    t.artist()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".into()),
                    t.album()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "Unknown".into()),
                    t.year()
                        .map(|y| y.to_string())
                        .unwrap_or_else(|| "?".into()),
                    t.genre()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "?".into()),
                )
            } else {
                (
                    name,
                    "Unknown".into(),
                    "Unknown".into(),
                    "?".into(),
                    "?".into(),
                )
            };
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_uppercase();

            let accent = Style::default().fg(SH_FN);
            let label = Style::default().fg(SH_CMT);
            let value = Style::default().fg(SH_FG);
            let sep = Style::default().fg(SH_CMT);

            let row = |k: &'static str, v: String| -> Line<'static> {
                Line::from(vec![
                    Span::styled(format!("  {:<10}: ", k), label),
                    Span::styled(v, value),
                ])
            };

            lines.push(Line::from(Span::styled("🎵  Audio File", accent)));
            lines.push(Line::from(Span::styled("─".repeat(44), sep)));
            lines.push(row("Title", title));
            lines.push(row("Artist", artist));
            lines.push(row("Album", album));
            lines.push(row("Year", year));
            lines.push(row("Genre", genre));
            lines.push(Line::from(Span::styled("─".repeat(44), sep)));
            lines.push(row("Format", ext));
            lines.push(row("Duration", format!("{:02}:{:02}", mins, secs)));
            lines.push(row("Bitrate", bitrate));
            lines.push(row("Sample", sample_rate));
            lines.push(row("Channels", channels));
            lines.push(Line::from(Span::styled("─".repeat(44), sep)));
            lines.push(Line::from(Span::styled(
                "  🎧 Open with your audio player",
                Style::default().fg(SH_CMT),
            )));
        }
        Some(Err(e)) => {
            lines.push(Line::from(Span::styled(
                "🎵  Audio file",
                Style::default().fg(SH_FN),
            )));
            lines.push(Line::from(Span::styled(
                format!("⚠  Could not read tags: {}", e),
                Style::default().fg(Color::Red),
            )));
        }
        None => {
            lines.push(Line::from(Span::styled(
                "🎵  Audio file",
                Style::default().fg(SH_FN),
            )));
            lines.push(Line::from(Span::styled(
                "⚠  Couldn't read this file's tags (it may be corrupted)",
                Style::default().fg(Color::Red),
            )));
        }
    }

    PreviewContent::Highlighted(lines)
}

// ── PDF ───────────────────────────────────────────────────────────────────────

#[cfg(windows)]
fn run_silenced<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    unsafe {
        let stdout_h = GetStdHandle(STD_OUTPUT_HANDLE);
        let stderr_h = GetStdHandle(STD_ERROR_HANDLE);

        if let Ok(null_file) = File::create("NUL") {
            let null_raw = null_file.as_raw_handle() as *mut std::ffi::c_void;
            SetStdHandle(STD_OUTPUT_HANDLE, null_raw);
            SetStdHandle(STD_ERROR_HANDLE, null_raw);
            let res = f();
            SetStdHandle(STD_OUTPUT_HANDLE, stdout_h);
            SetStdHandle(STD_ERROR_HANDLE, stderr_h);
            res
        } else {
            f()
        }
    }
}

#[cfg(not(windows))]
fn run_silenced<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}

fn get_pdf_cache_path(p: &PathBuf, page_idx: usize) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(p.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(p) {
        hasher.update(meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified() {
            if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                hasher.update(duration.as_nanos().to_le_bytes());
            }
        }
    }
    hasher.update(page_idx.to_le_bytes());
    std::env::temp_dir().join(format!("rr_pdf_{}.png", hex::encode(hasher.finalize())))
}

fn try_render_pdf_visual(p: &PathBuf, page_idx: usize) -> Result<Option<PreviewContent>, ()> {
    let cache_path = get_pdf_cache_path(p, page_idx);
    if cache_path.exists() {
        return Ok(Some(render_image(&cache_path, 0, false)));
    }
    if OFFICE_RENDER_FAILURES
        .lock()
        .get(&cache_path)
        .is_some_and(|failed_at| failed_at.elapsed() < std::time::Duration::from_secs(30))
    {
        return Err(());
    }
    {
        let mut pending = VISUAL_RENDER_PENDING.lock();
        if !pending.insert(cache_path.clone()) {
            return Ok(None);
        }
    }
    let source = p.clone();
    std::thread::spawn(move || {
        let generated = render_office_pdf_page(&source, &cache_path, page_idx);
        VISUAL_RENDER_PENDING.lock().remove(&cache_path);
        if generated {
            OFFICE_RENDER_FAILURES.lock().remove(&cache_path);
        } else {
            OFFICE_RENDER_FAILURES
                .lock()
                .insert(cache_path, std::time::Instant::now());
        }
        *PREVIEW_CACHE.lock() = None;
    });
    Ok(None)
}

fn render_pdf(
    p: &PathBuf,
    pdf_mode: crate::state::PdfRenderMode,
    page_idx: usize,
) -> PreviewContent {
    if pdf_mode == crate::state::PdfRenderMode::Visual {
        match try_render_pdf_visual(p, page_idx) {
            Ok(Some(content)) => return content,
            Ok(None) => {
                return PreviewContent::Highlighted(vec![
                    Line::from(Span::styled(
                        "PDF Visual Preview",
                        Style::default().fg(SH_FN).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        "Rendering first page in the background…",
                        Style::default().fg(SH_CMT),
                    )),
                ]);
            }
            Err(()) => {
                return PreviewContent::Highlighted(vec![
                    Line::from(Span::styled(
                        "PDF visual renderer unavailable",
                        Style::default().fg(Color::Red),
                    )),
                    Line::from(Span::styled(
                        "Switch PDF to Text mode to read extracted content.",
                        Style::default().fg(SH_CMT),
                    )),
                ]);
            }
        }
    }
    let mut lines = meta_header(p);
    match fs::read(p) {
        Ok(bytes) => {
            // Pre-check if PDF is encrypted/password-protected
            let is_encrypted = bytes.windows(8).any(|window| window == b"/Encrypt");
            if is_encrypted {
                lines.push(Line::from(Span::styled(
                    "📄  PDF Document (Encrypted)",
                    Style::default().fg(SH_FN),
                )));
                lines.push(Line::from(Span::styled(
                    "─".repeat(50),
                    Style::default().fg(SH_CMT),
                )));
                lines.push(Line::from(Span::styled(
                    "⚠  This PDF is password-protected or encrypted. Preview not available.",
                    Style::default().fg(Color::Red),
                )));
                return PreviewContent::Highlighted(lines);
            }

            match safe_parse(move || run_silenced(|| pdf_extract::extract_text_from_mem(&bytes))) {
                Some(Ok(text)) => {
                    lines.push(Line::from(vec![
                        Span::styled("📄  PDF Document", Style::default().fg(SH_FN)),
                        Span::styled(" [Text Mode]", Style::default().fg(SH_CMT)),
                    ]));
                    lines.push(Line::from(Span::styled(
                        "─".repeat(50),
                        Style::default().fg(SH_CMT),
                    )));
                    if text.trim().is_empty() {
                        lines.push(Line::from(Span::styled(
                            "  No extractable text (may be scanned/image-based)",
                            Style::default().fg(SH_CMT),
                        )));
                    } else {
                        for l in text.lines().filter(|l| !l.trim().is_empty()) {
                            lines.push(highlight_document_line(l));
                        }
                    }
                }
                Some(Err(e)) => {
                    lines.push(Line::from(Span::styled(
                        format!("⚠  PDF error: {}", e),
                        Style::default().fg(Color::Red),
                    )));
                }
                None => {
                    lines.push(Line::from(Span::styled(
                        "⚠  Couldn't read this PDF (it may be password-protected or corrupted)",
                        Style::default().fg(Color::Red),
                    )));
                }
            }
        }
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠  Cannot read: {}", e),
                Style::default().fg(Color::Red),
            )));
        }
    }
    PreviewContent::Highlighted(lines)
}

// ── EXACT OFFICE RENDERING ────────────────────────────────────────────────────

fn get_office_cache_path(p: &PathBuf, page_idx: usize) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(p.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(p) {
        hasher.update(meta.len().to_le_bytes());
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                hasher.update(d.as_nanos().to_le_bytes());
            }
        }
    }
    hasher.update(page_idx.to_le_bytes());
    let hash = hex::encode(hasher.finalize());
    std::env::temp_dir().join(format!("rr_office_{}.png", hash))
}

fn try_render_embedded_office_thumbnail(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
) -> Option<PreviewContent> {
    let file = fs::File::open(p).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    for name in [
        "docProps/thumbnail.jpeg",
        "docProps/thumbnail.jpg",
        "docProps/thumbnail.png",
        "docProps/Thumbnail.jpeg",
    ] {
        let mut entry = match archive.by_name(name) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).ok()?;
        let raw = image::load_from_memory(&bytes).ok()?;
        let rotated = match rotation {
            90 => raw.rotate90(),
            180 => raw.rotate180(),
            270 => raw.rotate270(),
            _ => raw,
        };
        let image = if flip_h { rotated.fliph() } else { rotated };
        let dimensions = Some((image.width(), image.height()));
        let image = Arc::new(image);
        *IMG_CACHE.lock() = Some(ImgCache {
            path: p.clone(),
            rotation,
            flip_h,
            img: Arc::clone(&image),
        });
        return Some(PreviewContent::ImageFallback(ImageFallbackInfo {
            path: p.clone(),
            dimensions,
            img: Some(image),
        }));
    }
    None
}

fn run_command_with_timeout(command: &mut Command, timeout: std::time::Duration) -> bool {
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn office_suite_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(configured) = std::env::var("RUSTY_RANGER_OFFICE") {
        candidates.push(PathBuf::from(configured));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(dir) = executable.parent() {
            candidates
                .push(dir.join("LibreOfficePortable\\App\\libreoffice\\program\\soffice.exe"));
            candidates.push(dir.join("OpenOfficePortable\\App\\openoffice\\program\\soffice.exe"));
        }
    }
    for variable in ["ProgramW6432", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
        if let Ok(root) = std::env::var(variable) {
            let root = PathBuf::from(root);
            candidates.push(root.join("LibreOffice\\program\\soffice.exe"));
            candidates.push(root.join("OpenOffice 4\\program\\soffice.exe"));
            candidates.push(root.join("Apache OpenOffice 4\\program\\soffice.exe"));
        }
    }
    candidates.extend([
        PathBuf::from("C:\\Program Files\\LibreOffice\\program\\soffice.exe"),
        PathBuf::from("C:\\Program Files (x86)\\LibreOffice\\program\\soffice.exe"),
        PathBuf::from("C:\\Program Files\\OpenOffice 4\\program\\soffice.exe"),
        PathBuf::from("C:\\Program Files (x86)\\OpenOffice 4\\program\\soffice.exe"),
        PathBuf::from("soffice"),
        PathBuf::from("libreoffice"),
    ]);
    candidates.dedup();
    candidates
}

fn convert_with_office_suite(
    source: &std::path::Path,
    work_dir: &std::path::Path,
    format: &str,
) -> Option<PathBuf> {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    );
    let output_dir = work_dir.join(format!("office-output-{unique}"));
    let profile_dir = work_dir.join(format!("office-profile-{unique}"));
    fs::create_dir_all(&output_dir).ok()?;
    fs::create_dir_all(&profile_dir).ok()?;
    let profile_uri = format!(
        "file:///{}",
        profile_dir.to_string_lossy().replace('\\', "/")
    );

    for candidate in office_suite_candidates() {
        let mut command = Command::new(candidate);
        command
            .arg("--headless")
            .arg("--nologo")
            .arg("--nodefault")
            .arg("--nolockcheck")
            .arg("--nofirststartwizard")
            .arg("--norestore")
            .arg(format!("-env:UserInstallation={profile_uri}"))
            .args(["--convert-to", format, "--outdir"])
            .arg(&output_dir)
            .arg(source)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if !run_command_with_timeout(&mut command, std::time::Duration::from_secs(30)) {
            continue;
        }
        let mut converted = fs::read_dir(&output_dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(format))
            })
            .collect::<Vec<_>>();
        converted.sort_by_key(|path| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0));
        if let Some(path) = converted.pop() {
            return Some(path);
        }
    }
    None
}

fn render_office_pdf_page(
    pdf_path: &std::path::Path,
    png_path: &std::path::Path,
    page_idx: usize,
) -> bool {
    let output_prefix = png_path.with_extension("");
    let page_number = page_idx.saturating_add(1).to_string();
    let mut poppler_candidates = vec![
        (PathBuf::from("pdftoppm"), PathBuf::from("pdftocairo")),
        (
            PathBuf::from("C:\\Program Files\\poppler\\Library\\bin\\pdftoppm.exe"),
            PathBuf::from("C:\\Program Files\\poppler\\Library\\bin\\pdftocairo.exe"),
        ),
        (
            PathBuf::from("C:\\Program Files\\poppler\\bin\\pdftoppm.exe"),
            PathBuf::from("C:\\Program Files\\poppler\\bin\\pdftocairo.exe"),
        ),
        (
            PathBuf::from("C:\\ProgramData\\chocolatey\\bin\\pdftoppm.exe"),
            PathBuf::from("C:\\ProgramData\\chocolatey\\bin\\pdftocairo.exe"),
        ),
    ];
    if let Some(home) = dirs::home_dir() {
        let bin = home.join(
            ".cache\\codex-runtimes\\codex-primary-runtime\\dependencies\\native\\poppler\\Library\\bin",
        );
        poppler_candidates.push((bin.join("pdftoppm.exe"), bin.join("pdftocairo.exe")));
    }
    for (pdftoppm, pdftocairo) in poppler_candidates {
        let mut command = Command::new(&pdftoppm);
        command
            .arg("-f")
            .arg(&page_number)
            .arg("-l")
            .arg(&page_number)
            .arg("-singlefile")
            .arg("-png")
            .arg("-r")
            .arg("144")
            .arg(pdf_path)
            .arg(&output_prefix)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if run_command_with_timeout(&mut command, std::time::Duration::from_secs(20))
            && png_path.exists()
        {
            return true;
        }

        let mut command = Command::new(&pdftocairo);
        command
            .arg("-f")
            .arg(&page_number)
            .arg("-l")
            .arg(&page_number)
            .arg("-singlefile")
            .arg("-png")
            .arg("-r")
            .arg("144")
            .arg(pdf_path)
            .arg(&output_prefix)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if run_command_with_timeout(&mut command, std::time::Duration::from_secs(20))
            && png_path.exists()
        {
            return true;
        }
    }

    for mutool in [
        PathBuf::from("mutool"),
        PathBuf::from("C:\\Program Files\\MuPDF\\mutool.exe"),
        PathBuf::from("C:\\ProgramData\\chocolatey\\bin\\mutool.exe"),
    ] {
        let mut command = Command::new(mutool);
        command
            .args(["draw", "-q", "-F", "png", "-r", "144", "-o"])
            .arg(png_path)
            .arg(pdf_path)
            .arg(&page_number)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if run_command_with_timeout(&mut command, std::time::Duration::from_secs(20))
            && png_path.exists()
        {
            return true;
        }
    }

    // LibreOffice Draw and Apache OpenOffice are the portable final
    // fallback. Both preserve embedded Tamil fonts while exporting the first
    // PDF page to a bitmap on systems without Poppler/MuPDF.
    let work_dir = png_path.with_extension("office-pdf-work");
    if page_idx == 0 {
        if let Some(converted) = convert_with_office_suite(pdf_path, &work_dir, "png") {
            if fs::copy(converted, png_path).is_ok() && png_path.exists() {
                return true;
            }
        }
    }
    false
}

fn try_render_office_exact(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    page_idx: usize,
) -> Option<PreviewContent> {
    let cache_path = get_office_cache_path(p, page_idx);
    if cache_path.exists() {
        return Some(render_image(&cache_path, rotation, flip_h));
    }
    if OFFICE_RENDER_FAILURES
        .lock()
        .get(&cache_path)
        .is_some_and(|failed_at| failed_at.elapsed() < std::time::Duration::from_secs(30))
    {
        return None;
    }
    {
        let mut pending = VISUAL_RENDER_PENDING.lock();
        if !pending.insert(cache_path.clone()) {
            return None;
        }
    }
    let source = p.clone();
    std::thread::spawn(move || {
        let _ = generate_office_exact_sync(&source, rotation, flip_h, page_idx);
        VISUAL_RENDER_PENDING.lock().remove(&cache_path);
        *PREVIEW_CACHE.lock() = None;
    });
    None
}

fn generate_office_exact_sync(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    page_idx: usize,
) -> Option<PreviewContent> {
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let cache_path = get_office_cache_path(p, page_idx);

    if cache_path.exists() {
        return Some(render_image(&cache_path, rotation, flip_h));
    }
    {
        let cache = IMG_CACHE.lock();
        if let Some(entry) = cache.as_ref() {
            if page_idx == 0
                && &entry.path == p
                && entry.rotation == rotation
                && entry.flip_h == flip_h
            {
                return Some(PreviewContent::ImageFallback(ImageFallbackInfo {
                    path: p.clone(),
                    dimensions: Some((entry.img.width(), entry.img.height())),
                    img: Some(Arc::clone(&entry.img)),
                }));
            }
        }
    }
    {
        let failures = OFFICE_RENDER_FAILURES.lock();
        if failures
            .get(&cache_path)
            .is_some_and(|failed_at| failed_at.elapsed() < std::time::Duration::from_secs(30))
        {
            return None;
        }
    }

    let mut generated = false;
    let cache_stem = cache_path.file_stem().unwrap_or_default().to_string_lossy();
    let work_dir = std::env::temp_dir().join(format!("{}_work", cache_stem));
    let _ = fs::create_dir_all(&work_dir);
    let pdf_path = work_dir.join("document.pdf");

    // Full mode deliberately prefers LibreOffice / Apache OpenOffice. It is
    // headless, works without a signed-in desktop Office session, preserves
    // document fonts, and handles PPT/PPTX/DOC/XLS consistently. Microsoft
    // Office COM remains a compatibility fallback.
    if let Some(converted_pdf) = convert_with_office_suite(p, &work_dir, "pdf") {
        generated = render_office_pdf_page(&converted_pdf, &cache_path, page_idx);
    }

    if !generated && (ext == "ppt" || ext == "pptx") {
        // Keep the COM invocation aligned with the proven main-branch
        // implementation; slide collections are one-based.
        let script = format!(
            "$ErrorActionPreference = 'Stop'\n\
             $ppt = $null\n$pres = $null\n\
             try {{\n\
               $ppt = New-Object -ComObject PowerPoint.Application\n\
               $ppt.Visible = [Microsoft.Office.Core.MsoTriState]::msoFalse\n\
               $pres = $ppt.Presentations.Open('{}', [Microsoft.Office.Core.MsoTriState]::msoTrue, [Microsoft.Office.Core.MsoTriState]::msoFalse, [Microsoft.Office.Core.MsoTriState]::msoFalse)\n\
               $pres.Slides.Item({}).Export('{}', 'PNG')\n\
             }} finally {{\n\
               if ($pres -ne $null) {{ $pres.Close() }}\n\
               if ($ppt -ne $null) {{ $ppt.Quit() }}\n\
             }}\n",
            p.to_string_lossy().replace("'", "''"),
            page_idx.saturating_add(1),
            cache_path.to_string_lossy().replace("'", "''")
        );
        let ps_path = work_dir.join("powerpoint_export.ps1");
        if fs::write(&ps_path, script).is_ok() {
            let mut command = Command::new("powershell");
            command
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    ps_path.to_str().unwrap(),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if run_command_with_timeout(&mut command, std::time::Duration::from_secs(35))
                && cache_path.exists()
            {
                generated = true;
            }
        }
    }

    if !generated && (ext == "doc" || ext == "docx" || ext == "xls" || ext == "xlsx") {
        let escaped_input = p.to_string_lossy().replace("'", "''");
        let escaped_pdf = pdf_path.to_string_lossy().replace("'", "''");
        let script = if ext == "doc" || ext == "docx" {
            format!(
                "$ErrorActionPreference = 'Stop'\n$word = $null\n$doc = $null\n\
                 try {{\n$word = New-Object -ComObject Word.Application\n$word.Visible = $false\n\
                 $doc = $word.Documents.Open('{}', $false, $true)\n\
                 $doc.ExportAsFixedFormat('{}', 17)\n\
                 }} finally {{ if ($doc -ne $null) {{ $doc.Close(0) }}; if ($word -ne $null) {{ $word.Quit() }} }}",
                escaped_input, escaped_pdf
            )
        } else {
            format!(
                "$ErrorActionPreference = 'Stop'\n$excel = $null\n$book = $null\n\
                 try {{\n$excel = New-Object -ComObject Excel.Application\n$excel.Visible = $false\n\
                 $book = $excel.Workbooks.Open('{}', 0, $true)\n\
                 $book.ExportAsFixedFormat(0, '{}')\n\
                 }} finally {{ if ($book -ne $null) {{ $book.Close($false) }}; if ($excel -ne $null) {{ $excel.Quit() }} }}",
                escaped_input, escaped_pdf
            )
        };
        let ps_path = work_dir.join("office_export.ps1");
        if fs::write(&ps_path, script).is_ok() {
            let mut command = Command::new("powershell");
            command
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    ps_path.to_str().unwrap(),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let _ = run_command_with_timeout(&mut command, std::time::Duration::from_secs(35));
            if pdf_path.exists() {
                generated = render_office_pdf_page(&pdf_path, &cache_path, page_idx);
            }
        }
    }

    if generated && cache_path.exists() {
        OFFICE_RENDER_FAILURES.lock().remove(&cache_path);
        Some(render_image(&cache_path, rotation, flip_h))
    } else {
        OFFICE_RENDER_FAILURES
            .lock()
            .insert(cache_path, std::time::Instant::now());
        None
    }
}

// ── DOCX ──────────────────────────────────────────────────────────────────────

fn render_docx(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    office_mode: crate::state::OfficeRenderMode,
) -> PreviewContent {
    if office_mode == crate::state::OfficeRenderMode::Full {
        if let Some(content) = try_render_office_exact(p, rotation, flip_h, 0) {
            return content;
        }
        if let Some(content) = try_render_embedded_office_thumbnail(p, rotation, flip_h) {
            return content;
        }
    }

    let mut lines = meta_header(p);
    let file = match fs::File::open(p) {
        Ok(f) => f,
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
    };
    let extracted: Option<Result<String, String>> = safe_parse(move || {
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut xml = String::new();
        for i in 0..archive.len() {
            if let Ok(mut entry) = archive.by_index(i) {
                if entry.name() == "word/document.xml" {
                    let _ = entry.read_to_string(&mut xml);
                    break;
                }
            }
        }
        Ok(xml)
    });

    let xml = match extracted {
        Some(Ok(xml)) => xml,
        Some(Err(e)) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ Not a valid DOCX: {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
        None => {
            lines.push(Line::from(Span::styled(
                "⚠  Couldn't read this document (it may be password-protected or corrupted)",
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
    };

    lines.push(Line::from(Span::styled(
        if office_mode == crate::state::OfficeRenderMode::Full {
            "📘  Word Document  ·  Portable structured preview"
        } else {
            "📘  Word Document  ·  Text mode"
        },
        Style::default().fg(SH_FN),
    )));
    lines.push(Line::from(Span::styled(
        "─".repeat(50),
        Style::default().fg(SH_CMT),
    )));

    let mut last_was_empty = false;
    for l in xml_text(&xml).trim().lines() {
        let is_empty = l.trim().is_empty();
        if is_empty {
            if last_was_empty {
                continue;
            }
            last_was_empty = true;
            lines.push(Line::from(""));
        } else {
            last_was_empty = false;
            lines.push(highlight_document_line(l));
        }
    }
    PreviewContent::Highlighted(lines)
}

// ── PPTX ──────────────────────────────────────────────────────────────────────

fn get_ppt_media_cache_path(p: &PathBuf, slide_idx: usize) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(p.to_string_lossy().as_bytes());
    hasher.update(slide_idx.to_le_bytes());
    if let Ok(meta) = fs::metadata(p) {
        hasher.update(meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified() {
            if let Ok(epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
                hasher.update(epoch.as_nanos().to_le_bytes());
            }
        }
    }
    std::env::temp_dir().join(format!(
        "rr_ppt_media_{}.png",
        hex::encode(hasher.finalize())
    ))
}

fn relationship_targets(xml: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for marker in ["Target=\"", "Target='"] {
        let quote = marker.chars().last().unwrap_or('"');
        for remainder in xml.split(marker).skip(1) {
            if let Some(end) = remainder.find(quote) {
                let target = &remainder[..end];
                if target.to_ascii_lowercase().contains("media/") {
                    targets.push(target.to_string());
                }
            }
        }
    }
    targets
}

fn try_render_pptx_slide_media(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    slide_idx: usize,
) -> Option<PreviewContent> {
    if p.extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("pptx"))
    {
        return None;
    }

    let cache_path = get_ppt_media_cache_path(p, slide_idx);
    if cache_path.exists() {
        return Some(render_image(&cache_path, rotation, flip_h));
    }

    let file = fs::File::open(p).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let rels_name = format!(
        "ppt/slides/_rels/slide{}.xml.rels",
        slide_idx.saturating_add(1),
    );
    let mut relationships = String::new();
    archive
        .by_name(&rels_name)
        .ok()?
        .read_to_string(&mut relationships)
        .ok()?;

    let mut best: Option<(u64, image::DynamicImage)> = None;
    for target in relationship_targets(&relationships) {
        let Some(file_name) = target
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .map(str::to_string)
        else {
            continue;
        };
        let media_name = format!("ppt/media/{file_name}");
        let mut bytes = Vec::new();
        let Ok(mut entry) = archive.by_name(&media_name) else {
            continue;
        };
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        let Ok(image) = image::load_from_memory(&bytes) else {
            continue;
        };
        let area = u64::from(image.width()) * u64::from(image.height());
        if best.as_ref().is_none_or(|(best_area, _)| area > *best_area) {
            best = Some((area, image));
        }
    }

    let (_, image) = best?;
    image.save(&cache_path).ok()?;
    Some(render_image(&cache_path, rotation, flip_h))
}

fn render_pptx(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    office_mode: crate::state::OfficeRenderMode,
    slide_idx: usize,
) -> PreviewContent {
    if office_mode == crate::state::OfficeRenderMode::Full {
        if let Some(content) = try_render_office_exact(p, rotation, flip_h, slide_idx) {
            return content;
        }
        if slide_idx == 0 {
            if let Some(content) = try_render_embedded_office_thumbnail(p, rotation, flip_h) {
                return content;
            }
        }
        if let Some(content) = try_render_pptx_slide_media(p, rotation, flip_h, slide_idx) {
            return content;
        }
    }

    let mut lines = meta_header(p);
    let file = match fs::File::open(p) {
        Ok(f) => f,
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
    };

    let slides: Option<Vec<(u32, String)>> = safe_parse(move || {
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(_) => return Vec::new(),
        };
        let mut slide_numbers = (0..archive.len())
            .filter_map(|index| {
                archive
                    .by_index(index)
                    .ok()
                    .map(|entry| entry.name().to_string())
            })
            .filter_map(|name| {
                name.strip_prefix("ppt/slides/slide")
                    .and_then(|value| value.strip_suffix(".xml"))
                    .and_then(|value| value.parse::<u32>().ok())
            })
            .collect::<Vec<_>>();
        slide_numbers.sort_unstable();
        slide_numbers.dedup();

        let mut out = Vec::new();
        for slide_n in slide_numbers {
            let name = format!("ppt/slides/slide{}.xml", slide_n);
            let mut xml = String::new();
            if archive
                .by_name(&name)
                .ok()
                .is_some_and(|mut entry| entry.read_to_string(&mut xml).is_ok())
            {
                out.push((slide_n, xml));
            }
        }
        out
    });

    let Some(slides) = slides else {
        lines.push(Line::from(Span::styled(
            "⚠  Couldn't read this presentation (it may be password-protected or corrupted)",
            Style::default().fg(Color::Red),
        )));
        return PreviewContent::Highlighted(lines);
    };

    if slides.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Empty presentation",
            Style::default().fg(SH_CMT),
        )));
        return PreviewContent::Highlighted(lines);
    }

    let slide_count = slides.len();
    lines.push(Line::from(vec![
        Span::styled(
            "📊  PowerPoint",
            Style::default().fg(SH_FN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if office_mode == crate::state::OfficeRenderMode::Full {
                format!(
                    "  ·  Portable slide preview  ·  {} / {}",
                    slide_idx.saturating_add(1).min(slide_count),
                    slide_count,
                )
            } else {
                format!("  ·  Text outline  ·  {} slides", slide_count)
            },
            Style::default().fg(SH_CMT),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "─".repeat(72),
        Style::default().fg(SH_CMT),
    )));

    let visible_slides: Box<dyn Iterator<Item = &(u32, String)>> =
        if office_mode == crate::state::OfficeRenderMode::Full {
            let index = slide_idx.min(slide_count.saturating_sub(1));
            Box::new(slides[index..=index].iter())
        } else {
            Box::new(slides.iter())
        };

    for (slide_n, xml) in visible_slides {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" SLIDE {} ", slide_n),
            Style::default()
                .fg(Color::Black)
                .bg(SH_TYPE)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        let extracted_text = xml_text(&xml);
        let mut slide_has_text = false;
        for l in extracted_text.lines() {
            let trimmed = l.trim();
            if !trimmed.is_empty() {
                lines.push(highlight_document_line(trimmed));
                slide_has_text = true;
            }
        }
        if !slide_has_text {
            lines.push(Line::from(Span::styled(
                "  (Slide content visual only)",
                Style::default().fg(SH_CMT),
            )));
        }
    }
    PreviewContent::Highlighted(lines)
}

// ── Excel ─────────────────────────────────────────────────────────────────────

fn render_excel(
    p: &PathBuf,
    rotation: u32,
    flip_h: bool,
    office_mode: crate::state::OfficeRenderMode,
) -> PreviewContent {
    if office_mode == crate::state::OfficeRenderMode::Full {
        if let Some(content) = try_render_office_exact(p, rotation, flip_h, 0) {
            return content;
        }
        if let Some(content) = try_render_embedded_office_thumbnail(p, rotation, flip_h) {
            return content;
        }
    }

    use calamine::{open_workbook_auto, Reader};
    let mut lines = meta_header(p);
    let outcome: Option<Vec<Line<'static>>> = safe_parse(move || {
        let mut out: Vec<Line<'static>> = Vec::new();
        match open_workbook_auto(p) {
            Ok(mut wb) => {
                let sheets = wb.sheet_names().to_vec();
                out.push(Line::from(vec![
                    Span::styled(
                        if office_mode == crate::state::OfficeRenderMode::Full {
                            "📊  Spreadsheet  ·  Portable grid  ·  Sheets: "
                        } else {
                            "📊  Spreadsheet  ·  Sheets: "
                        },
                        Style::default().fg(SH_FN),
                    ),
                    Span::styled(sheets.join(", "), Style::default().fg(SH_STR)),
                ]));
                out.push(Line::from(Span::styled(
                    "─".repeat(80),
                    Style::default().fg(SH_CMT),
                )));

                if let Some(first) = sheets.first() {
                    if let Ok(range) = wb.worksheet_range(first) {
                        for (ri, row) in range.rows().enumerate() {
                            let formatted: String = row
                                .iter()
                                .map(|c| {
                                    let s = c.to_string();
                                    format!(" {:<16} ", safe_truncate(&s, 15))
                                })
                                .collect::<Vec<_>>()
                                .join("│");
                            if ri == 0 {
                                out.push(Line::from(Span::styled(
                                    formatted.clone(),
                                    Style::default().fg(SH_TYPE).add_modifier(Modifier::BOLD),
                                )));
                                out.push(Line::from(Span::styled(
                                    "─".repeat(formatted.len()),
                                    Style::default().fg(SH_CMT),
                                )));
                            } else {
                                let color = if ri % 2 == 0 {
                                    SH_FG
                                } else {
                                    Color::Rgb(170, 175, 196)
                                };
                                out.push(Line::from(Span::styled(
                                    formatted,
                                    Style::default().fg(color),
                                )));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                out.push(Line::from(Span::styled(
                    format!("⚠ Cannot read spreadsheet: {}", e),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        out
    });

    match outcome {
        Some(out) => lines.extend(out),
        None => lines.push(Line::from(Span::styled(
            "⚠  Couldn't read this spreadsheet (it may be password-protected or corrupted)",
            Style::default().fg(Color::Red),
        ))),
    }
    PreviewContent::Code(Arc::new(lines))
}

// ── CSV / TSV ─────────────────────────────────────────────────────────────────

fn render_csv(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let delim = if ext == "tsv" { b'\t' } else { b',' };

    let file = match fs::File::open(p) {
        Ok(f) => f,
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
    };

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(false)
        .flexible(true)
        .from_reader(file);

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut total_rows = 0usize;
    for result in rdr.records() {
        total_rows += 1;
        if rows.len() < 50 {
            if let Ok(record) = result {
                rows.push(record.iter().map(|s| s.to_string()).collect());
            }
        }
    }

    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "📊  Empty CSV",
            Style::default().fg(SH_FN),
        )));
        return PreviewContent::Highlighted(lines);
    }

    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; ncols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < ncols {
                col_widths[i] = col_widths[i].max(cell.len().min(18));
            }
        }
    }

    lines.push(Line::from(vec![
        Span::styled("📊  CSV  ", Style::default().fg(SH_FN)),
        Span::styled(
            format!("({} rows × {} cols)", total_rows, ncols),
            Style::default().fg(SH_CMT),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "─".repeat(44),
        Style::default().fg(SH_CMT),
    )));

    for (ri, row) in rows.iter().enumerate() {
        let cells: Vec<Span<'static>> = (0..ncols)
            .flat_map(|ci| {
                let cell = row.get(ci).map(|s| s.as_str()).unwrap_or("");
                let w = col_widths[ci].max(1);
                let s = format!(" {:<width$} ", safe_truncate(cell, 18), width = w);
                let style = if ri == 0 {
                    Style::default().fg(SH_TYPE).add_modifier(Modifier::BOLD)
                } else if ci % 2 == 0 {
                    Style::default().fg(SH_FG)
                } else {
                    Style::default().fg(Color::Rgb(170, 175, 196))
                };
                vec![
                    Span::styled(s, style),
                    Span::styled("│", Style::default().fg(SH_CMT)),
                ]
            })
            .collect();
        lines.push(Line::from(cells));
        if ri == 0 {
            let sep_w: usize = col_widths.iter().map(|w| w + 3).sum();
            lines.push(Line::from(Span::styled(
                "─".repeat(sep_w.min(80)),
                Style::default().fg(SH_CMT),
            )));
        }
    }
    if total_rows > 50 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more rows", total_rows - 50),
            Style::default().fg(SH_CMT),
        )));
    }
    PreviewContent::Highlighted(lines)
}

// ── Jupyter Notebook (.ipynb) ─────────────────────────────────────────────────

fn render_notebook(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let content = match fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Code(Arc::new(lines));
        }
    };

    let nb: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ Not valid JSON: {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Code(Arc::new(lines));
        }
    };

    let kernel = nb["metadata"]["kernelspec"]["display_name"]
        .as_str()
        .unwrap_or("?");
    let cells = match nb["cells"].as_array() {
        Some(c) => c,
        None => {
            lines.push(Line::from(Span::styled(
                "Notebook contains no cells",
                Style::default().fg(SH_CMT),
            )));
            return PreviewContent::Code(Arc::new(lines));
        }
    };

    lines.push(Line::from(vec![
        Span::styled(
            "JUPYTER NOTEBOOK",
            Style::default().fg(SH_FN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  |  Kernel: {}  |  {} cells", kernel, cells.len()),
            Style::default().fg(SH_CMT),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "─".repeat(72),
        Style::default().fg(SH_CMT),
    )));

    for (i, cell) in cells.iter().enumerate() {
        let ctype = cell["cell_type"].as_str().unwrap_or("?");
        let source: String = match cell["source"].as_array() {
            Some(ls) => ls.iter().filter_map(|l| l.as_str()).collect(),
            None => cell["source"].as_str().unwrap_or("").to_string(),
        };

        let (badge, badge_color) = match ctype {
            "code" => ("CODE", SH_FN),
            "markdown" => ("MARKDOWN", SH_TYPE),
            _ => ("RAW", SH_CMT),
        };
        let execution = cell["execution_count"]
            .as_u64()
            .map(|value| value.to_string())
            .unwrap_or_else(|| " ".to_string());

        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", badge),
                Style::default()
                    .fg(Color::Black)
                    .bg(badge_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if ctype == "code" {
                    format!(" In [{execution}]  ·  Cell {}", i + 1)
                } else {
                    format!("  Cell {}", i + 1)
                },
                Style::default().fg(SH_CMT),
            ),
        ]));

        if ctype == "code" {
            lines.extend(highlight_code(&source, "py"));
        } else {
            for line in source.lines() {
                lines.push(if line.trim().is_empty() {
                    Line::from("")
                } else {
                    highlight_document_line(line)
                });
            }
        }

        // Render textual outputs as their own compact block. Rich binary MIME
        // payloads are intentionally omitted; image output is not readable as
        // terminal text and often contains megabytes of base64.
        if let Some(outputs) = cell["outputs"].as_array() {
            for out in outputs {
                let txt: String = out["text"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|l| l.as_str()).collect())
                    .or_else(|| {
                        out["data"]["text/plain"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|l| l.as_str()).collect())
                    })
                    .or_else(|| out["data"]["text/plain"].as_str().map(str::to_string))
                    .unwrap_or_default();
                if !txt.is_empty() {
                    lines.push(Line::from(Span::styled(
                        " OUTPUT ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(SH_KW)
                            .add_modifier(Modifier::BOLD),
                    )));
                    for output_line in txt.lines() {
                        lines.push(Line::from(vec![
                            Span::styled("> ", Style::default().fg(SH_CMT)),
                            Span::styled(output_line.to_string(), Style::default().fg(SH_STR)),
                        ]));
                    }
                }
            }
        }
    }

    PreviewContent::Code(Arc::new(lines))
}

// ── RTF ───────────────────────────────────────────────────────────────────────

fn render_rtf(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let bytes = match fs::read(p) {
        Ok(b) => b,
        Err(e) => {
            lines.push(Line::from(Span::styled(
                format!("⚠ {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
    };
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                while let Some(&nc) = chars.peek() {
                    if nc.is_alphabetic() || nc == '-' {
                        chars.next();
                    } else if nc.is_ascii_digit() {
                        chars.next();
                    } else {
                        if nc == ' ' {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            '{' | '}' => {}
            '\n' | '\r' => {
                out.push('\n');
            }
            _ => {
                out.push(ch);
            }
        }
    }

    lines.push(Line::from(Span::styled(
        "📄  RTF Document",
        Style::default().fg(SH_FN),
    )));
    lines.push(Line::from(Span::styled(
        "─".repeat(44),
        Style::default().fg(SH_CMT),
    )));
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(300) {
        lines.push(highlight_document_line(l));
    }
    PreviewContent::Highlighted(lines)
}

// ── Archive ───────────────────────────────────────────────────────────────────

fn render_archive(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "zip" {
        let listing: Option<Vec<Line<'static>>> = safe_parse(move || {
            let mut out = Vec::new();
            if let Ok(Ok(mut a)) = fs::File::open(p).map(zip::ZipArchive::new) {
                let total = a.len();
                out.push(Line::from(vec![
                    Span::styled("📦  ZIP  ", Style::default().fg(SH_FN)),
                    Span::styled(format!("{} entries", total), Style::default().fg(SH_CMT)),
                ]));
                out.push(Line::from(Span::styled(
                    "─".repeat(44),
                    Style::default().fg(SH_CMT),
                )));
                for i in 0..total.min(150) {
                    if let Ok(f) = a.by_index(i) {
                        let is_dir = f.is_dir();
                        let icon = if is_dir { "📂" } else { "📄" };
                        let name = f.name().to_string();
                        let orig = human_size(f.size());
                        let comp = human_size(f.compressed_size());
                        out.push(Line::from(vec![
                            Span::styled(format!("  {} ", icon), Style::default().fg(SH_FG)),
                            Span::styled(
                                format!("{:<50}", safe_truncate(&name, 50)),
                                Style::default().fg(if is_dir { SH_TYPE } else { SH_FG }),
                            ),
                            Span::styled(
                                format!(" {:>8} → {}", comp, orig),
                                Style::default().fg(SH_CMT),
                            ),
                        ]));
                    }
                }
                if total > 150 {
                    out.push(Line::from(Span::styled(
                        format!("  … {} more entries", total - 150),
                        Style::default().fg(SH_CMT),
                    )));
                }
            }
            out
        });
        if let Some(out) = listing {
            if !out.is_empty() {
                lines.extend(out);
                return PreviewContent::Highlighted(lines);
            }
        }
    }

    let label = format!("📦  {} Archive", ext.to_uppercase());
    lines.push(Line::from(Span::styled(label, Style::default().fg(SH_FN))));
    lines.push(Line::from(Span::styled(
        "  Use 7-Zip or similar to inspect this archive.",
        Style::default().fg(SH_CMT),
    )));
    PreviewContent::Highlighted(lines)
}

// ── Text / code ───────────────────────────────────────────────────────────────

fn text_preview(p: &PathBuf) -> PreviewContent {
    let header = meta_header(p);
    let bytes = match fs::read(p) {
        Ok(b) => b,
        Err(e) => {
            let mut lines = header;
            lines.push(Line::from(Span::styled(
                format!("⚠ {}", e),
                Style::default().fg(Color::Red),
            )));
            return PreviewContent::Highlighted(lines);
        }
    };

    // Binary detection: null bytes in first 8KB
    let sample = &bytes[..bytes.len().min(8192)];
    if sample.contains(&0u8) {
        let hex_lines = render_hex_lines(&bytes);
        let mut lines = header;
        lines.push(Line::from(Span::styled(
            format!("⚙  Binary  ({} bytes)", bytes.len()),
            Style::default().fg(SH_TYPE),
        )));
        lines.push(Line::from(Span::styled(
            "─".repeat(70),
            Style::default().fg(SH_CMT),
        )));
        lines.push(Line::from(vec![
            Span::styled("Offset    ", Style::default().fg(SH_CMT)),
            Span::styled(
                "00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  ",
                Style::default().fg(SH_OP),
            ),
            Span::styled("ASCII", Style::default().fg(SH_STR)),
        ]));
        lines.extend(hex_lines);
        return PreviewContent::Highlighted(lines);
    }

    let text = String::from_utf8(bytes.clone())
        .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned());
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Use syntax highlighting for code files
    if is_code(&ext) && !normalized.is_empty() {
        let mut lines = header;
        lines.extend(highlight_code(&normalized, &ext));
        return PreviewContent::Code(Arc::new(lines));
    }

    let total = normalized.lines().count();
    let gutter_digits = total.max(1).to_string().len();
    let mut lines = header;
    if normalized.is_empty() {
        lines.push(Line::from(Span::styled(
            "<empty file>",
            Style::default().fg(SH_CMT),
        )));
    } else {
        lines.extend(normalized.lines().enumerate().map(|(index, line)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>width$}", index + 1, width = gutter_digits),
                    Style::default().fg(SH_LN),
                ),
                Span::styled("│ ", Style::default().fg(SH_CMT)),
                Span::styled(line.to_string(), Style::default().fg(SH_FG)),
            ])
        }));
    }
    PreviewContent::Code(Arc::new(lines))
}

// ── Hex dump lines ────────────────────────────────────────────────────────────

fn render_hex_lines(bytes: &[u8]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (chunk_i, chunk) in bytes[..bytes.len().min(512)].chunks(16).enumerate() {
        let offset = chunk_i * 16;
        let hex_part: String = chunk
            .chunks(8)
            .map(|g| {
                g.iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("  ");
        let ascii_part: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '·'
                }
            })
            .collect();
        lines.push(Line::from(vec![
            Span::styled(format!("{:08X}  ", offset), Style::default().fg(SH_CMT)),
            Span::styled(format!("{:<47}  ", hex_part), Style::default().fg(SH_OP)),
            Span::styled(ascii_part, Style::default().fg(SH_STR)),
        ]));
    }
    if bytes.len() > 512 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more bytes", bytes.len() - 512),
            Style::default().fg(SH_CMT),
        )));
    }
    lines
}

fn is_code(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "py"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "sh"
            | "bash"
            | "zsh"
            | "ps1"
            | "bat"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "java"
            | "kt"
            | "go"
            | "rb"
            | "php"
            | "cs"
            | "lua"
            | "sql"
            | "xml"
            | "md"
            | "markdown"
            | "log"
            | "ini"
            | "cfg"
            | "conf"
            | "swift"
            | "dart"
            | "r"
            | "jl"
            | "hs"
            | "ex"
            | "exs"
            | "vue"
            | "svelte"
            | "astro"
            | "dockerfile"
            | "makefile"
            | "gitignore"
            | "env"
            | "txt"
    )
}

// ── XML text extractor ────────────────────────────────────────────────────────

fn xml_text(xml: &str) -> String {
    let mut out = String::new();
    let b = xml.as_bytes();
    let mut i = 0;
    let mut in_tag = false;
    let mut tag = String::new();
    let mut collect = false;

    while i < b.len() {
        match b[i] {
            b'<' => {
                in_tag = true;
                tag.clear();
                i += 1;
            }
            b'>' if in_tag => {
                in_tag = false;
                let t = tag.trim_start_matches('/');
                let name = t.split_whitespace().next().unwrap_or("");
                collect = matches!(name, "w:t" | "a:t" | "t" | "w:delText");
                if matches!(name, "w:p" | "/w:p" | "a:p" | "/a:p" | "w:br") {
                    out.push('\n');
                }
                i += 1;
            }
            _ if in_tag => {
                tag.push(b[i] as char);
                i += 1;
            }
            _ if collect => {
                out.push(b[i] as char);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

// ── Save file (edit mode) ─────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn save_file(path: &PathBuf, content: &str) -> anyhow::Result<()> {
    fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn full_office_preview_uses_embedded_thumbnail() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-office-preview-{}-{}.pptx",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 3)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();

        let file = fs::File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("docProps/thumbnail.png", zip::write::FileOptions::default())
            .unwrap();
        archive.write_all(png.get_ref()).unwrap();
        archive.finish().unwrap();

        match try_render_embedded_office_thumbnail(&path, 0, false) {
            Some(PreviewContent::ImageFallback(info)) => {
                assert_eq!(info.dimensions, Some((4, 3)));
                assert!(info.img.is_some());
            }
            _ => panic!("expected embedded Office thumbnail preview"),
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn counts_only_real_presentation_slides() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-slide-count-{}-{}.pptx",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = fs::File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for name in [
            "ppt/slides/slide1.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/slide3.xml",
            "ppt/slides/_rels/slide1.xml.rels",
        ] {
            archive
                .start_file(name, zip::write::FileOptions::default())
                .unwrap();
            archive.write_all(b"<xml/>").unwrap();
        }
        archive.finish().unwrap();

        assert_eq!(presentation_slide_count(&path), Some(3));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn portable_full_presentation_preview_shows_only_the_selected_slide() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-portable-slides-{}-{}.pptx",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = fs::File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (number, text) in [(1, "FIRST SLIDE"), (2, "SECOND SLIDE"), (3, "THIRD SLIDE")] {
            archive
                .start_file(
                    format!("ppt/slides/slide{number}.xml"),
                    zip::write::FileOptions::default(),
                )
                .unwrap();
            archive
                .write_all(format!("<a:p><a:t>{text}</a:t></a:p>").as_bytes())
                .unwrap();
        }
        archive.finish().unwrap();

        let cache_path = get_office_cache_path(&path, 1);
        OFFICE_RENDER_FAILURES
            .lock()
            .insert(cache_path.clone(), std::time::Instant::now());
        match render_pptx(&path, 0, false, crate::state::OfficeRenderMode::Full, 1) {
            PreviewContent::Highlighted(lines) => {
                let rendered = lines
                    .iter()
                    .flat_map(|line| line.spans.iter())
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                assert!(rendered.contains("Portable slide preview"));
                assert!(rendered.contains("SECOND SLIDE"));
                assert!(!rendered.contains("FIRST SLIDE"));
                assert!(!rendered.contains("renderer unavailable"));
            }
            _ => panic!("missing portable slide preview"),
        }
        OFFICE_RENDER_FAILURES.lock().remove(&cache_path);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unknown_text_files_use_a_separate_line_number_gutter() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-plain-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, "alpha\nbeta").unwrap();

        match text_preview(&path) {
            PreviewContent::Code(lines) => {
                assert_eq!(lines.len(), 2);
                assert_eq!(lines[0].spans[0].content.as_ref(), "1");
                assert_eq!(lines[0].spans[1].content.as_ref(), "│ ");
                assert_eq!(lines[0].spans[2].content.as_ref(), "alpha");
                assert_ne!(lines[0].spans[0].style.fg, lines[0].spans[2].style.fg);
            }
            _ => panic!("plain text should render with a styled gutter"),
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn code_gutter_expands_once_and_stays_aligned_past_line_100() {
        let source = (1..=105)
            .map(|line| format!("value_{line} = {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = highlight_code_with_limit(&source, "py", 400);

        assert_eq!(lines[0].spans[0].content.as_ref(), "  1│ ");
        assert_eq!(lines[98].spans[0].content.as_ref(), " 99│ ");
        assert_eq!(lines[99].spans[0].content.as_ref(), "100│ ");
        assert_eq!(lines[104].spans[0].content.as_ref(), "105│ ");
        assert!(lines.iter().all(|line| line.spans[0].width() == 5));
    }

    #[test]
    fn read_only_code_preview_keeps_every_line() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-long-preview-{}-{}.py",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = (1..=710)
            .map(|line| format!("value_{line} = {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, source).unwrap();

        match text_preview(&path) {
            PreviewContent::Code(lines) => {
                assert_eq!(lines.len(), 710);
                assert_eq!(lines[709].spans[0].content.as_ref(), "710│ ");
            }
            _ => panic!("Python should use the full code preview"),
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn notebook_preview_is_unwrapped_and_keeps_complete_cells_and_output() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-notebook-preview-{}-{}.ipynb",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let notebook = serde_json::json!({
            "metadata": { "kernelspec": { "display_name": "Python 3" } },
            "cells": [{
                "cell_type": "code",
                "execution_count": 7,
                "source": ["first = 1\n", "second = 2\n", "third = first + second"],
                "outputs": [{ "text": ["one\n", "two\n", "three\n", "four\n"] }]
            }, {
                "cell_type": "markdown",
                "source": ["# Complete markdown cell\n", "Final line"]
            }]
        });
        fs::write(&path, serde_json::to_vec(&notebook).unwrap()).unwrap();

        match render_notebook(&path) {
            PreviewContent::Code(lines) => {
                let rendered = lines
                    .iter()
                    .flat_map(|line| line.spans.iter())
                    .map(|span| span.content.as_ref())
                    .collect::<String>();
                assert!(rendered.contains("In [7]"));
                assert!(rendered.contains("third"));
                assert!(rendered.contains("four"));
                assert!(rendered.contains("Complete markdown cell"));
                assert!(!rendered.contains("more cells"));
            }
            _ => panic!("notebooks must use the non-wrapping code viewport"),
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn cached_notebook_frames_share_the_parsed_line_buffer() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-notebook-cache-{}-{}.ipynb",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let notebook = serde_json::json!({
            "metadata": { "kernelspec": { "display_name": "Python 3" } },
            "cells": [{
                "cell_type": "code",
                "execution_count": 1,
                "source": (1..=500).map(|line| format!("value_{line} = {line}\n")).collect::<Vec<_>>(),
                "outputs": []
            }]
        });
        fs::write(&path, serde_json::to_vec(&notebook).unwrap()).unwrap();

        let first = render(
            &path,
            0,
            false,
            crate::state::OfficeRenderMode::Text,
            crate::state::PdfRenderMode::Text,
            0,
        );
        let second = render(
            &path,
            0,
            false,
            crate::state::OfficeRenderMode::Text,
            crate::state::PdfRenderMode::Text,
            0,
        );
        match (first, second) {
            (PreviewContent::Code(first), PreviewContent::Code(second)) => {
                assert!(Arc::ptr_eq(&first, &second));
            }
            _ => panic!("cached notebook must remain a shared code viewport"),
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn powerpoint_relationship_parser_finds_slide_media() {
        let relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
            <Relationships>
              <Relationship Id="rId1" Target="../media/image1.png"/>
              <Relationship Id="rId2" Target="../charts/chart1.xml"/>
              <Relationship Id="rId3" Target='../media/image2.jpeg'/>
            </Relationships>"#;

        assert_eq!(
            relationship_targets(relationships),
            vec![
                "../media/image1.png".to_string(),
                "../media/image2.jpeg".to_string()
            ]
        );
    }

    #[test]
    fn preview_text_is_normalized_for_tamil_combining_marks() {
        use unicode_normalization::UnicodeNormalization;

        let decomposed = "\u{0B95}\u{0BC6}\u{0BBE}";
        let expected = decomposed.nfc().collect::<String>();
        let content = normalize_preview_content(PreviewContent::Highlighted(vec![Line::from(
            Span::raw(decomposed.to_string()),
        )]));

        match content {
            PreviewContent::Highlighted(lines) => {
                assert_eq!(lines[0].spans[0].content.as_ref(), expected);
            }
            _ => panic!("Tamil text must remain a highlighted text preview"),
        }
    }

    #[test]
    fn tamil_graphemes_are_never_truncated_between_combining_marks() {
        use unicode_segmentation::UnicodeSegmentation;

        let text = "கொடுத்தல்";
        let first = safe_truncate(text, 1);
        assert!(text.starts_with(first));
        assert_eq!(first.graphemes(true).count(), 1);
    }

    #[test]
    fn visual_cache_key_changes_when_source_size_changes() {
        let path = std::env::temp_dir().join(format!(
            "rusty-ranger-visual-cache-key-{}-{}.pdf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"a").unwrap();
        let first_pdf = get_pdf_cache_path(&path, 0);
        let first_office = get_office_cache_path(&path, 0);
        fs::write(&path, b"same-second-update").unwrap();
        assert_ne!(first_pdf, get_pdf_cache_path(&path, 0));
        assert_ne!(first_office, get_office_cache_path(&path, 0));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn office_candidates_include_libreoffice_and_openoffice() {
        let candidates = office_suite_candidates()
            .iter()
            .map(|path| path.to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();
        assert!(candidates.iter().any(|path| path.contains("libreoffice")));
        assert!(candidates.iter().any(|path| path.contains("openoffice")));
    }

    #[test]
    fn video_cache_is_unique_per_source_and_supports_common_formats() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-video-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = base.with_extension("mp4");
        let second = base.with_extension("m2ts");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();

        assert_ne!(get_video_cache_path(&first), get_video_cache_path(&second));
        assert!(is_video_extension("mp4"));
        assert!(is_video_extension("m2ts"));
        assert!(is_video_extension("webm"));
        assert!(!is_video_extension("ts"));
        assert!(!is_video_extension("txt"));

        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
    }

    #[test]
    fn pdf_visual_backend_produces_a_real_image_when_available() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-pdf-visual-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pdf = base.with_extension("pdf");
        let png = base.with_extension("png");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>",
            "<< /Length 22 >>\nstream\n0 0 1 rg 20 20 160 160 re f\nendstream",
        ];
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes
                .extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                objects.len() + 1,
                xref
            )
            .as_bytes(),
        );
        fs::write(&pdf, bytes).unwrap();

        if render_office_pdf_page(&pdf, &png, 0) {
            let image = image::open(&png).expect("rendered PDF page should be a valid image");
            assert!(image.width() > 0 && image.height() > 0);
        }

        let _ = fs::remove_file(pdf);
        let _ = fs::remove_file(png);
    }
}
