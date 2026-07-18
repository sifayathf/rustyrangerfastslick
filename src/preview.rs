// ================= src/preview.rs =================
// Cross-platform file preview engine.
//
// PreviewContent enum:
//   Text        → plain Paragraph
//   Highlighted → colored spans (syntax-highlighted code)
//   Image       → ImageWidget (Buffer half-block rendering)

use std::{
    path::PathBuf,
    fs,
    process::Command,
    sync::Arc,
    io::Read,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use ratatui::{
    text::{Line, Span},
    style::{Style, Color, Modifier},
};

// ── Public types ──────────────────────────────────────────────────────────────

pub struct ImageFallbackInfo {
    pub path: PathBuf,
    pub dimensions: Option<(u32, u32)>,
    pub img: Option<std::sync::Arc<image::DynamicImage>>,
}

pub enum PreviewContent {
    Text(String),
    Highlighted(Vec<Line<'static>>),
    ImageFallback(ImageFallbackInfo),
}

// ── Syntax-highlight color palette (Catppuccin Mocha) ────────────────────────
const SH_KW:   Color = Color::Rgb(203, 166, 247); // mauve   — keywords
const SH_STR:  Color = Color::Rgb(166, 227, 161); // green   — strings
const SH_CMT:  Color = Color::Rgb(108, 112, 134); // overlay — comments
const SH_NUM:  Color = Color::Rgb(250, 179, 135); // peach   — numbers
const SH_OP:   Color = Color::Rgb(137, 180, 250); // blue    — operators
const SH_FG:   Color = Color::Rgb(205, 214, 244); // text    — default
const SH_LN:   Color = Color::Rgb(88,  91,  112); // surface — line numbers
const SH_TYPE: Color = Color::Rgb(249, 226, 175); // yellow  — types / builtins
const SH_FN:   Color = Color::Rgb(116, 199, 236); // sky     — function names

// ── Image decode + transform cache ───────────────────────────────────────────

struct ImgCache {
    path:     PathBuf,
    rotation: u32,
    flip_h:   bool,
    img:      Arc<image::DynamicImage>,
}

static IMG_CACHE: Lazy<Mutex<Option<ImgCache>>> = Lazy::new(|| Mutex::new(None));

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
        90  => raw.rotate90(),
        180 => raw.rotate180(),
        270 => raw.rotate270(),
        _   => raw,
    };
    let final_img = if flip_h { rotated.fliph() } else { rotated };
    let arc = Arc::new(final_img);
    *IMG_CACHE.lock() = Some(ImgCache {
        path: path.clone(), rotation, flip_h, img: Arc::clone(&arc)
    });
    Some(arc)
}

// ── Directory listing cache ───────────────────────────────────────────────────

struct DirCache {
    path:    PathBuf,
    entries: Vec<crate::state::DirEntryInfo>,
}

static DIR_CACHE: Lazy<Mutex<Option<DirCache>>> = Lazy::new(|| Mutex::new(None));

pub fn cached_list_dir(path: &PathBuf) -> Vec<crate::state::DirEntryInfo> {
    {
        let g = DIR_CACHE.lock();
        if let Some(c) = g.as_ref() {
            if &c.path == path { return c.entries.clone(); }
        }
    }
    let entries = crate::state::list_dir(path).unwrap_or_default();
    *DIR_CACHE.lock() = Some(DirCache { path: path.clone(), entries: entries.clone() });
    entries
}

// ── Public render function ────────────────────────────────────────────────────

pub fn render(p: &PathBuf, rotation: u32, flip_h: bool) -> PreviewContent {
    if p.is_dir() { return PreviewContent::Text(String::new()); }

    let ext = p.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg"|"jpeg"|"png"|"bmp"|"gif"|"webp"|"tiff"|"tif"|"ico"
            => render_image(p, rotation, flip_h),
        "mp4"|"mkv"|"avi"|"mov"|"webm"|"flv"|"wmv"|"m4v"
            => render_video(p),
        "mp3"|"flac"|"wav"|"ogg"|"aac"|"m4a"|"opus"|"wma"
            => render_audio(p),
        "docx"|"doc"   => render_docx(p, rotation, flip_h),
        "xlsx"|"xls"|"ods" => render_excel(p, rotation, flip_h),
        "pptx"|"ppt"   => render_pptx(p, rotation, flip_h),
        "pdf"           => render_pdf(p),
        "rtf"           => render_rtf(p),
        "csv"|"tsv"     => render_csv(p),
        "ipynb"         => render_notebook(p),
        "zip"|"7z"|"tar"|"gz"|"bz2"|"xz"|"rar"|"tgz"
            => render_archive(p),
        _   => text_preview(p),
    }
}

// ── Metadata header ───────────────────────────────────────────────────────────

pub fn meta_header(_p: &PathBuf) -> Vec<Line<'static>> {
    Vec::new()
}

pub fn human_size(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    for &u in UNITS {
        if v < 1024.0 { return format!("{:.1} {}", v, u); }
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
    let is_likely_heading = trimmed.len() < 50 
        && !trimmed.ends_with('.') 
        && !trimmed.ends_with(',')
        && !trimmed.contains(':')
        && trimmed.chars().any(|c| c.is_alphabetic())
        && (trimmed.chars().filter(|c| c.is_uppercase()).count() as f32 / trimmed.len() as f32 > 0.2 || trimmed.len() < 25);

    if is_likely_heading {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        ));
    }

    // 2. Check for list items / bullets
    if trimmed.starts_with('•') || trimmed.starts_with('-') || trimmed.starts_with('*') || 
       (trimmed.chars().next().map_or(false, |c| c.is_numeric()) && trimmed.contains(". ")) {
        // Find the index of the bullet/number separator
        let sep_idx = if trimmed.starts_with('•') || trimmed.starts_with('-') || trimmed.starts_with('*') {
            1
        } else {
            trimmed.find(". ").unwrap() + 2
        };
        
        let bullet = &trimmed[..sep_idx];
        let content = &trimmed[sep_idx..];
        
        return Line::from(vec![
            Span::styled(format!("  {} ", bullet), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(content.to_string(), Style::default().fg(Color::Rgb(205, 214, 244))),
        ]);
    }

    // 3. Normal paragraph text - check for emails/websites to highlight
    if trimmed.contains('@') || trimmed.contains("http://") || trimmed.contains("https://") || trimmed.contains("www.") {
        let mut spans = Vec::new();
        for word in line.split_whitespace() {
            if word.contains('@') || word.contains("://") || word.starts_with("www.") {
                spans.push(Span::styled(format!("{} ", word), Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)));
            } else {
                spans.push(Span::styled(format!("{} ", word), Style::default().fg(Color::Rgb(205, 214, 244))));
            }
        }
        return Line::from(spans);
    }

    // Default: soft white paragraph text
    Line::from(Span::styled(
        line.to_string(),
        Style::default().fg(Color::Rgb(205, 214, 244))
    ))
}

// ── Syntax highlighting ───────────────────────────────────────────────────────

fn highlight_code(text: &str, ext: &str) -> Vec<Line<'static>> {
    let keywords  = language_keywords(ext);
    let types_bi  = language_types(ext);
    let cmt_line  = line_comment_prefix(ext);
    let cmt_block = block_comment_chars(ext); // (open, close)

    let mut lines: Vec<Line<'static>> = Vec::new();

    // We need cross-line block-comment tracking
    let mut in_block_cmt = false;

    for (ln_idx, raw_line) in text.lines().enumerate().take(400) {
        let mut spans: Vec<Span<'static>> = Vec::new();

        // Line number gutter
        spans.push(Span::styled(
            format!("{:>4} │ ", ln_idx + 1),
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
            if chars[i] == '"' || chars[i] == '`'
                || (chars[i] == '\'' && ext != "rs") // skip lifetime 'a in Rust
            {
                let quote = chars[i];
                let start = i;
                i += 1;
                while i < n {
                    if chars[i] == '\\' { i += 2; continue; } // escape
                    if chars[i] == quote { i += 1; break; }
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
                if chars[i] == '0' && i + 1 < n && (chars[i+1] == 'x' || chars[i+1] == 'X') {
                    i += 2;
                    while i < n && (chars[i].is_ascii_hexdigit() || chars[i] == '_') { i += 1; }
                } else {
                    while i < n && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_'
                        || chars[i] == 'e' || chars[i] == 'E') { i += 1; }
                    // type suffix like u32, f64
                    while i < n && (chars[i].is_alphabetic() || chars[i] == '_') { i += 1; }
                }
                let s: String = chars[start..i].iter().collect();
                spans.push(Span::styled(s, Style::default().fg(SH_NUM)));
                continue;
            }

            // Identifiers / keywords / types
            if chars[i].is_alphabetic() || chars[i] == '_' {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') { i += 1; }
                let word: String = chars[start..i].iter().collect();

                // Check next char: if '(' it's a function call
                let next_non_ws = chars[i..].iter().find(|&&c| c != ' ');
                let is_fn_call  = next_non_ws == Some(&'(');

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
            let s  = ch.to_string();
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
        "rs" => &["as","async","await","break","const","continue","crate","dyn",
                   "else","enum","extern","false","fn","for","if","impl","in",
                   "let","loop","match","mod","move","mut","pub","ref","return",
                   "self","Self","static","struct","super","trait","true","type",
                   "unsafe","use","where","while","box"],
        "py" => &["and","as","assert","async","await","break","class","continue",
                   "def","del","elif","else","except","finally","for","from",
                   "global","if","import","in","is","lambda","nonlocal","not",
                   "or","pass","raise","return","try","while","with","yield"],
        "js"|"ts"|"jsx"|"tsx" => &[
                   "abstract","as","async","await","break","case","catch","class",
                   "const","continue","debugger","default","delete","do","else",
                   "enum","export","extends","finally","for","from","function",
                   "if","import","in","instanceof","interface","let","new","of",
                   "package","private","protected","public","return","static",
                   "super","switch","this","throw","try","typeof","var","void",
                   "while","with","yield","type","declare","readonly","implements",
                   "namespace","module","satisfies","override"],
        "go" => &["break","case","chan","const","continue","default","defer","else",
                   "fallthrough","for","func","go","goto","if","import","interface",
                   "map","package","range","return","select","struct","switch","type",
                   "var"],
        "java"|"kt"|"kts" => &["abstract","break","case","catch","class","const",
                   "continue","default","do","else","enum","extends","final","finally",
                   "for","goto","if","implements","import","instanceof","interface",
                   "native","new","package","private","protected","public","return",
                   "static","strictfp","super","switch","synchronized","this","throw",
                   "throws","transient","try","var","void","volatile","while",
                   // Kotlin extras
                   "data","fun","in","is","object","open","override","sealed","when",
                   "companion","lateinit","inline","reified","suspend","coroutine"],
        "c"|"cpp"|"h"|"hpp"|"cc" => &[
                   "auto","break","case","char","const","continue","default","do",
                   "double","else","enum","extern","float","for","goto","if","inline",
                   "int","long","register","return","short","signed","sizeof","static",
                   "struct","switch","typedef","union","unsigned","void","volatile",
                   "while","class","delete","friend","new","operator","private",
                   "protected","public","template","this","throw","try","catch",
                   "virtual","override","final","nullptr","constexpr","decltype","auto"],
        "cs" => &["abstract","as","base","bool","break","byte","case","catch","char",
                   "checked","class","const","continue","decimal","default","delegate",
                   "do","double","else","enum","event","explicit","extern","false",
                   "finally","fixed","float","for","foreach","goto","if","implicit",
                   "in","int","interface","internal","is","lock","long","namespace",
                   "new","null","object","operator","out","override","params","private",
                   "protected","public","readonly","ref","return","sbyte","sealed",
                   "short","sizeof","stackalloc","static","string","struct","switch",
                   "this","throw","true","try","typeof","uint","ulong","unchecked",
                   "unsafe","ushort","using","virtual","void","volatile","while",
                   "async","await","dynamic","var","record","init","with"],
        "rb" => &["__FILE__","__LINE__","__ENCODING__","BEGIN","END","alias","and",
                   "begin","break","case","class","def","defined?","do","else","elsif",
                   "end","ensure","false","for","if","in","module","next","nil","not",
                   "or","redo","rescue","retry","return","self","super","then","true",
                   "undef","unless","until","when","while","yield"],
        "php" => &["abstract","and","array","as","break","callable","case","catch",
                    "class","clone","const","continue","declare","default","do","echo",
                    "else","elseif","empty","enddeclare","endfor","endforeach","endif",
                    "endswitch","endwhile","eval","exit","extends","final","finally",
                    "fn","for","foreach","function","global","goto","if","implements",
                    "include","include_once","instanceof","insteadof","interface",
                    "isset","list","match","namespace","new","null","or","print",
                    "private","protected","public","readonly","require","require_once",
                    "return","static","switch","throw","trait","try","true","unset",
                    "use","var","while","xor","yield","false"],
        "swift" => &["associatedtype","class","deinit","enum","extension","fileprivate",
                      "func","import","init","inout","internal","let","open","operator",
                      "private","precedencegroup","protocol","public","rethrows","static",
                      "struct","subscript","typealias","var","break","case","continue",
                      "default","defer","do","else","fallthrough","for","guard","if","in",
                      "repeat","return","throw","switch","where","while","as","catch",
                      "false","is","nil","rethrows","self","super","throw","throws","true",
                      "try","_"],
        "lua" => &["and","break","do","else","elseif","end","false","for","function",
                    "goto","if","in","local","nil","not","or","repeat","return","then",
                    "true","until","while"],
        "sh"|"bash"|"zsh" => &["if","then","else","elif","fi","for","while","do","done",
                                 "case","esac","function","return","local","export","echo",
                                 "read","exit","in","source","alias","unset","declare"],
        "sql" => &["select","from","where","join","left","right","inner","outer","on",
                    "group","order","by","having","limit","offset","insert","into",
                    "values","update","set","delete","create","table","index","view",
                    "drop","alter","add","column","primary","key","foreign","references",
                    "unique","not","null","default","and","or","in","like","between",
                    "exists","union","all","distinct","as","with","case","when","then",
                    "else","end","count","sum","avg","max","min","asc","desc"],
        "html"|"htm" => &["doctype","html","head","body","div","span","p","a","img",
                            "input","button","form","table","tr","td","th","thead","tbody",
                            "ul","ol","li","h1","h2","h3","h4","h5","h6","header","footer",
                            "nav","main","section","article","aside","script","style",
                            "link","meta","title","br","hr","strong","em","code","pre"],
        "css"|"scss" => &["important","media","keyframes","import","charset","font-face",
                            "supports","from","to","px","em","rem","vh","vw","auto",
                            "none","inherit","initial","unset","flex","grid","block",
                            "inline","absolute","relative","fixed","sticky"],
        _    => &[],
    };
    kws.iter().cloned().collect()
}

fn language_types(ext: &str) -> std::collections::HashSet<&'static str> {
    let types: &[&str] = match ext {
        "rs" => &["String","Vec","HashMap","HashSet","Option","Result","Box","Arc",
                   "Rc","Cell","RefCell","Mutex","RwLock","PathBuf","Path","OsStr",
                   "OsString","i8","i16","i32","i64","i128","isize","u8","u16","u32",
                   "u64","u128","usize","f32","f64","bool","char","str","()","Some",
                   "None","Ok","Err","true","false"],
        "py" => &["int","str","float","bool","list","dict","set","tuple","bytes",
                   "bytearray","None","True","False","type","object","super","print",
                   "len","range","enumerate","zip","map","filter","sorted","reversed",
                   "isinstance","issubclass","hasattr","getattr","setattr","delattr",
                   "TypeError","ValueError","KeyError","IndexError","AttributeError",
                   "Exception","RuntimeError","StopIteration","self","cls"],
        "ts"|"js"|"tsx"|"jsx" => &[
                   "string","number","boolean","null","undefined","never","any","unknown",
                   "void","object","symbol","bigint","Array","Promise","Record","Partial",
                   "Required","Readonly","Pick","Omit","true","false","null"],
        "go" => &["bool","byte","complex64","complex128","error","float32","float64",
                   "int","int8","int16","int32","int64","rune","string","uint","uint8",
                   "uint16","uint32","uint64","uintptr","true","false","nil","iota",
                   "append","cap","close","copy","delete","len","make","new","panic",
                   "print","println","recover"],
        _ => &[],
    };
    types.iter().cloned().collect()
}

fn line_comment_prefix(ext: &str) -> &'static str {
    match ext {
        "rs"|"js"|"ts"|"tsx"|"jsx"|"go"|"java"|"kt"|"kts"|"cs"|"swift"|"php"|"c"|"cpp"|"h"|"hpp"|"cc"
            => "//",
        "py"|"sh"|"bash"|"zsh"|"rb"|"toml"|"yaml"|"yml"|"dockerfile"|"gitignore"
            => "#",
        "lua"  => "--",
        "sql"  => "--",
        "html"|"htm" => "<!--",
        "css"|"scss" => "/*",   // handled as block comment
        _      => "",
    }
}

fn block_comment_chars(ext: &str) -> Option<(&'static str, &'static str)> {
    match ext {
        "rs"|"js"|"ts"|"tsx"|"jsx"|"go"|"java"|"kt"|"kts"|"cs"|"swift"|"c"|"cpp"|"h"|"hpp"|"cc"|"css"|"scss"
            => Some(("/*", "*/")),
        "html"|"htm" => Some(("<!--", "-->")),
        _    => None,
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

fn render_video(p: &PathBuf) -> PreviewContent {
    {
        let g = IMG_CACHE.lock();
        if let Some(c) = g.as_ref() {
            if &c.path == p { 
                return PreviewContent::ImageFallback(ImageFallbackInfo {
                    path: p.clone(),
                    dimensions: Some((c.img.width(), c.img.height())),
                    img: Some(Arc::clone(&c.img)),
                });
            }
        }
    }

    let thumb = std::env::temp_dir().join("rr_video_thumb.png");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "quiet",
               "-i", p.to_str().unwrap_or(""),
               "-ss", "00:00:02", "-vframes", "1",
               thumb.to_str().unwrap_or("")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok && thumb.exists() {
        if let Ok(img) = image::open(&thumb) {
            let (w, h) = (img.width(), img.height());
            let arc = Arc::new(img);
            *IMG_CACHE.lock() = Some(ImgCache {
                path: p.clone(), rotation: 0, flip_h: false, img: Arc::clone(&arc)
            });
            return PreviewContent::ImageFallback(ImageFallbackInfo {
                path: p.clone(),
                dimensions: Some((w, h)),
                img: Some(arc),
            });
        }
    }

    let mut lines = meta_header(p);
    lines.push(Line::from(Span::styled("🎬  Video file", Style::default().fg(SH_FN))));
    lines.push(Line::from(Span::styled("(Install ffmpeg for thumbnail preview)", Style::default().fg(SH_CMT))));
    PreviewContent::Highlighted(lines)
}

// ── Audio ─────────────────────────────────────────────────────────────────────

fn render_audio(p: &PathBuf) -> PreviewContent {
    use lofty::prelude::*;
    use lofty::probe::Probe;

    let mut lines = meta_header(p);
    let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();

    match Probe::open(p).and_then(|pr| pr.read()) {
        Ok(tagged_file) => {
            let props = tagged_file.properties();
            let duration = props.duration();
            let mins = duration.as_secs() / 60;
            let secs = duration.as_secs() % 60;
            let bitrate = props.audio_bitrate()
                .map(|b| format!("{} kbps", b)).unwrap_or_else(|| "?".into());
            let sample_rate = props.sample_rate()
                .map(|s| format!("{} Hz", s)).unwrap_or_else(|| "?".into());
            let channels = props.channels()
                .map(|c| c.to_string()).unwrap_or_else(|| "?".into());

            let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag());
            let (title, artist, album, year, genre) = if let Some(t) = tag {
                (
                    t.title().map(|s| s.to_string()).unwrap_or_else(|| name.clone()),
                    t.artist().map(|s| s.to_string()).unwrap_or_else(|| "Unknown".into()),
                    t.album().map(|s| s.to_string()).unwrap_or_else(|| "Unknown".into()),
                    t.year().map(|y| y.to_string()).unwrap_or_else(|| "?".into()),
                    t.genre().map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                )
            } else {
                (name, "Unknown".into(), "Unknown".into(), "?".into(), "?".into())
            };
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("?").to_uppercase();

            let accent = Style::default().fg(SH_FN);
            let label  = Style::default().fg(SH_CMT);
            let value  = Style::default().fg(SH_FG);
            let sep    = Style::default().fg(SH_CMT);

            let row = |k: &'static str, v: String| -> Line<'static> {
                Line::from(vec![
                    Span::styled(format!("  {:<10}: ", k), label),
                    Span::styled(v, value),
                ])
            };

            lines.push(Line::from(Span::styled("🎵  Audio File", accent)));
            lines.push(Line::from(Span::styled("─".repeat(44), sep)));
            lines.push(row("Title",   title));
            lines.push(row("Artist",  artist));
            lines.push(row("Album",   album));
            lines.push(row("Year",    year));
            lines.push(row("Genre",   genre));
            lines.push(Line::from(Span::styled("─".repeat(44), sep)));
            lines.push(row("Format",  ext));
            lines.push(row("Duration", format!("{:02}:{:02}", mins, secs)));
            lines.push(row("Bitrate", bitrate));
            lines.push(row("Sample",  sample_rate));
            lines.push(row("Channels", channels));
            lines.push(Line::from(Span::styled("─".repeat(44), sep)));
            lines.push(Line::from(Span::styled("  🎧 Open with your audio player", Style::default().fg(SH_CMT))));
        }
        Err(e) => {
            lines.push(Line::from(Span::styled("🎵  Audio file", Style::default().fg(SH_FN))));
            lines.push(Line::from(Span::styled(format!("⚠  Could not read tags: {}", e), Style::default().fg(Color::Red))));
        }
    }

    PreviewContent::Highlighted(lines)
}

// ── PDF ───────────────────────────────────────────────────────────────────────

fn render_pdf(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    match fs::read(p) {
        Ok(bytes) => match pdf_extract::extract_text_from_mem(&bytes) {
            Ok(text) => {
                lines.push(Line::from(Span::styled("📄  PDF Document", Style::default().fg(SH_FN))));
                lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(SH_CMT))));
                if text.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  No extractable text (may be scanned/image-based)",
                        Style::default().fg(SH_CMT)
                    )));
                } else {
                    for l in text.lines().filter(|l| !l.trim().is_empty()).take(300) {
                        lines.push(highlight_document_line(l));
                    }
                }
            }
            Err(e) => {
                lines.push(Line::from(Span::styled(format!("⚠  PDF error: {}", e), Style::default().fg(Color::Red))));
            }
        },
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠  Cannot read: {}", e), Style::default().fg(Color::Red))));
        }
    }
    PreviewContent::Highlighted(lines)
}

// ── EXACT OFFICE RENDERING ────────────────────────────────────────────────────

fn get_office_cache_path(p: &PathBuf) -> PathBuf {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(p.to_string_lossy().as_bytes());
    if let Ok(meta) = fs::metadata(p) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                hasher.update(d.as_secs().to_string().as_bytes());
            }
        }
    }
    let hash = hex::encode(hasher.finalize());
    std::env::temp_dir().join(format!("rr_office_{}.png", hash))
}

fn try_render_office_exact(p: &PathBuf, rotation: u32, flip_h: bool) -> Option<PreviewContent> {
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let cache_path = get_office_cache_path(p);

    if cache_path.exists() {
        return Some(render_image(&cache_path, rotation, flip_h));
    }

    let mut generated = false;

    if ext == "ppt" || ext == "pptx" {
        // Use PowerPoint COM to export the first slide to PNG
        let script = format!(
            "$ppt = New-Object -ComObject PowerPoint.Application\n\
             $ppt.Visible = [Microsoft.Office.Core.MsoTriState]::msoFalse\n\
             $pres = $ppt.Presentations.Open('{}', [Microsoft.Office.Core.MsoTriState]::msoTrue, [Microsoft.Office.Core.MsoTriState]::msoFalse, [Microsoft.Office.Core.MsoTriState]::msoFalse)\n\
             $pres.Slides.Item(1).Export('{}', 'PNG')\n\
             $pres.Close()\n\
             $ppt.Quit()\n",
            p.to_string_lossy().replace("'", "''"),
            cache_path.to_string_lossy().replace("'", "''")
        );
        let ps_path = std::env::temp_dir().join("rr_ppt_export.ps1");
        if fs::write(&ps_path, script).is_ok() {
            let status = Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps_path.to_str().unwrap()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if status.map_or(false, |s| s.success()) && cache_path.exists() {
                generated = true;
            }
            let _ = fs::remove_file(ps_path);
        }
    }

    if !generated && (ext == "doc" || ext == "docx" || ext == "xls" || ext == "xlsx" || ext == "ppt" || ext == "pptx") {
        // Try LibreOffice headless
        let soffice_paths = [
            "C:\\Program Files\\LibreOffice\\program\\soffice.exe",
            "C:\\Program Files (x86)\\LibreOffice\\program\\soffice.exe",
            "soffice",
        ];
        
        let temp_dir = std::env::temp_dir();
        for soffice in soffice_paths {
            let status = Command::new(soffice)
                .args(["--headless", "--convert-to", "png", "--outdir", temp_dir.to_str().unwrap(), p.to_str().unwrap()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
                
            if status.map_or(false, |s| s.success()) {
                // LibreOffice outputs to <temp_dir>/<filename>.png
                let base_name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let lo_out = temp_dir.join(format!("{}.png", base_name));
                if lo_out.exists() {
                    let _ = fs::rename(&lo_out, &cache_path);
                    generated = true;
                    break;
                }
            }
        }
    }

    if generated && cache_path.exists() {
        Some(render_image(&cache_path, rotation, flip_h))
    } else {
        None
    }
}

// ── DOCX ──────────────────────────────────────────────────────────────────────

fn render_docx(p: &PathBuf, rotation: u32, flip_h: bool) -> PreviewContent {
    if let Some(content) = try_render_office_exact(p, rotation, flip_h) {
        return content;
    }

    let mut lines = meta_header(p);
    let file = match fs::File::open(p) {
        Ok(f)  => f,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a)  => a,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ Not a valid DOCX: {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };

    let mut xml = String::new();
    for i in 0..archive.len() {
        if let Ok(mut entry) = archive.by_index(i) {
            if entry.name() == "word/document.xml" {
                let _ = entry.read_to_string(&mut xml);
                break;
            }
        }
    }

    lines.push(Line::from(Span::styled("📘  Word Document (Text Fallback)", Style::default().fg(SH_FN))));
    lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(SH_CMT))));
    
    let mut last_was_empty = false;
    for l in xml_text(&xml).trim().lines().take(300) {
        let is_empty = l.trim().is_empty();
        if is_empty {
            if last_was_empty { continue; }
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

fn render_pptx(p: &PathBuf, rotation: u32, flip_h: bool) -> PreviewContent {
    if let Some(content) = try_render_office_exact(p, rotation, flip_h) {
        return content;
    }

    let mut lines = meta_header(p);
    let file = match fs::File::open(p) {
        Ok(f)  => f,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a)  => a,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ Not a valid PPTX: {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };

    lines.push(Line::from(Span::styled("📊  PowerPoint (Text Fallback)", Style::default().fg(SH_FN))));
    lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(SH_CMT))));

    for slide_n in 1..=50u32 {
        let name = format!("ppt/slides/slide{}.xml", slide_n);
        let mut xml = String::new();
        let found = (0..archive.len()).any(|i| {
            archive.by_index(i).ok().map_or(false, |mut e| {
                if e.name() == name { let _ = e.read_to_string(&mut xml); true } else { false }
            })
        });
        if !found { break; }
        lines.push(Line::from(Span::styled(
            format!("── Slide {} ", slide_n), Style::default().fg(SH_TYPE)
        )));
        for l in xml_text(&xml).trim().lines().take(10) {
            if !l.trim().is_empty() {
                lines.push(highlight_document_line(l));
            }
        }
    }
    PreviewContent::Highlighted(lines)
}

// ── Excel ─────────────────────────────────────────────────────────────────────

fn render_excel(p: &PathBuf, rotation: u32, flip_h: bool) -> PreviewContent {
    if let Some(content) = try_render_office_exact(p, rotation, flip_h) {
        return content;
    }

    use calamine::{Reader, open_workbook_auto};
    let mut lines = meta_header(p);
    match open_workbook_auto(p) {
        Ok(mut wb) => {
            let sheets = wb.sheet_names().to_vec();
            lines.push(Line::from(vec![
                Span::styled("📊  Sheets: ", Style::default().fg(SH_FN)),
                Span::styled(sheets.join(", "), Style::default().fg(SH_STR)),
            ]));
            lines.push(Line::from(Span::styled("─".repeat(80), Style::default().fg(SH_CMT))));

            if let Some(first) = sheets.first() {
                if let Ok(range) = wb.worksheet_range(first) {
                    for (ri, row) in range.rows().enumerate().take(40) {
                        let formatted: String = row.iter()
                            .map(|c| { let s = c.to_string(); format!(" {:<16} ", &s[..s.len().min(15)]) })
                            .collect::<Vec<_>>()
                            .join("│");
                        if ri == 0 {
                            lines.push(Line::from(Span::styled(formatted.clone(), Style::default().fg(SH_TYPE).add_modifier(Modifier::BOLD))));
                            lines.push(Line::from(Span::styled("─".repeat(formatted.len()), Style::default().fg(SH_CMT))));
                        } else {
                            let color = if ri % 2 == 0 { SH_FG } else { Color::Rgb(170, 175, 196) };
                            lines.push(Line::from(Span::styled(formatted, Style::default().fg(color))));
                        }
                    }
                }
            }
        }
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ Cannot read spreadsheet: {}", e), Style::default().fg(Color::Red))));
        }
    }
    PreviewContent::Highlighted(lines)
}

// ── CSV / TSV ─────────────────────────────────────────────────────────────────

fn render_csv(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let delim = if ext == "tsv" { b'\t' } else { b',' };

    let file = match fs::File::open(p) {
        Ok(f)  => f,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim).has_headers(false).flexible(true).from_reader(file);

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
        lines.push(Line::from(Span::styled("📊  Empty CSV", Style::default().fg(SH_FN))));
        return PreviewContent::Highlighted(lines);
    }

    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; ncols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < ncols { col_widths[i] = col_widths[i].max(cell.len().min(18)); }
        }
    }

    lines.push(Line::from(vec![
        Span::styled("📊  CSV  ", Style::default().fg(SH_FN)),
        Span::styled(format!("({} rows × {} cols)", total_rows, ncols), Style::default().fg(SH_CMT)),
    ]));
    lines.push(Line::from(Span::styled("─".repeat(44), Style::default().fg(SH_CMT))));

    for (ri, row) in rows.iter().enumerate() {
        let cells: Vec<Span<'static>> = (0..ncols).flat_map(|ci| {
            let cell = row.get(ci).map(|s| s.as_str()).unwrap_or("");
            let w = col_widths[ci].max(1);
            let s = format!(" {:<width$} ", &cell[..cell.len().min(18)], width = w);
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
        }).collect();
        lines.push(Line::from(cells));
        if ri == 0 {
            let sep_w: usize = col_widths.iter().map(|w| w + 3).sum();
            lines.push(Line::from(Span::styled("─".repeat(sep_w.min(80)), Style::default().fg(SH_CMT))));
        }
    }
    if total_rows > 50 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more rows", total_rows - 50),
            Style::default().fg(SH_CMT)
        )));
    }
    PreviewContent::Highlighted(lines)
}

// ── Jupyter Notebook (.ipynb) ─────────────────────────────────────────────────

fn render_notebook(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let content = match fs::read_to_string(p) {
        Ok(s)  => s,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };

    let nb: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v)  => v,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ Not valid JSON: {}", e), Style::default().fg(Color::Red))));
            return PreviewContent::Highlighted(lines);
        }
    };

    let kernel = nb["metadata"]["kernelspec"]["display_name"].as_str().unwrap_or("?");
    let cells = match nb["cells"].as_array() {
        Some(c) => c,
        None    => {
            lines.push(Line::from(Span::styled("📓  No cells found", Style::default().fg(SH_CMT))));
            return PreviewContent::Highlighted(lines);
        }
    };

    lines.push(Line::from(vec![
        Span::styled("📓  Jupyter  ", Style::default().fg(SH_FN)),
        Span::styled(format!("Kernel: {}  ({} cells)", kernel, cells.len()), Style::default().fg(SH_CMT)),
    ]));

    for (i, cell) in cells.iter().enumerate().take(60) {
        let ctype = cell["cell_type"].as_str().unwrap_or("?");
        let source: String = match cell["source"].as_array() {
            Some(ls) => ls.iter().filter_map(|l| l.as_str()).collect(),
            None     => cell["source"].as_str().unwrap_or("").to_string(),
        };

        let (badge, badge_color) = match ctype {
            "code"     => ("💻 CODE    ", SH_FN),
            "markdown" => ("📝 MARKDOWN", SH_TYPE),
            _          => ("📋 RAW     ", SH_CMT),
        };

        lines.push(Line::from(Span::styled("─".repeat(44), Style::default().fg(SH_CMT))));
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", badge), Style::default().fg(badge_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("[{}]", i + 1), Style::default().fg(SH_CMT)),
        ]));

        if ctype == "code" {
            // syntax highlight the code cell using Python rules
            let cell_lines = highlight_code(&source, "py");
            lines.extend(cell_lines.into_iter().take(20));
        } else {
            for l in source.lines().take(10) {
                if !l.trim().is_empty() {
                    lines.push(highlight_document_line(l));
                }
            }
        }

        // Outputs
        if let Some(outputs) = cell["outputs"].as_array() {
            for out in outputs.iter().take(2) {
                let txt: String = out["text"].as_array()
                    .map(|a| a.iter().filter_map(|l| l.as_str()).collect())
                    .or_else(|| out["data"]["text/plain"].as_array()
                        .map(|a| a.iter().filter_map(|l| l.as_str()).collect()))
                    .unwrap_or_default();
                if !txt.is_empty() {
                    lines.push(Line::from(Span::styled("  Out► ", Style::default().fg(SH_KW))));
                    for ol in txt.lines().take(3) {
                        lines.push(Line::from(Span::styled(format!("    {}", ol), Style::default().fg(SH_STR))));
                    }
                }
            }
        }
    }

    if cells.len() > 60 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more cells", cells.len() - 60),
            Style::default().fg(SH_CMT)
        )));
    }
    PreviewContent::Highlighted(lines)
}

// ── RTF ───────────────────────────────────────────────────────────────────────

fn render_rtf(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let bytes = match fs::read(p) {
        Ok(b)  => b,
        Err(e) => {
            lines.push(Line::from(Span::styled(format!("⚠ {}", e), Style::default().fg(Color::Red))));
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
                    if nc.is_alphabetic() || nc == '-' { chars.next(); }
                    else if nc.is_ascii_digit() { chars.next(); }
                    else { if nc == ' ' { chars.next(); } break; }
                }
            }
            '{' | '}' => {}
            '\n' | '\r' => { out.push('\n'); }
            _ => { out.push(ch); }
        }
    }

    lines.push(Line::from(Span::styled("📄  RTF Document", Style::default().fg(SH_FN))));
    lines.push(Line::from(Span::styled("─".repeat(44), Style::default().fg(SH_CMT))));
    for l in out.lines().filter(|l| !l.trim().is_empty()).take(300) {
        lines.push(highlight_document_line(l));
    }
    PreviewContent::Highlighted(lines)
}

// ── Archive ───────────────────────────────────────────────────────────────────

fn render_archive(p: &PathBuf) -> PreviewContent {
    let mut lines = meta_header(p);
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

    if ext == "zip" {
        match fs::File::open(p).map(zip::ZipArchive::new) {
            Ok(Ok(mut a)) => {
                let total = a.len();
                lines.push(Line::from(vec![
                    Span::styled("📦  ZIP  ", Style::default().fg(SH_FN)),
                    Span::styled(format!("{} entries", total), Style::default().fg(SH_CMT)),
                ]));
                lines.push(Line::from(Span::styled("─".repeat(44), Style::default().fg(SH_CMT))));
                for i in 0..total.min(150) {
                    if let Ok(f) = a.by_index(i) {
                        let is_dir  = f.is_dir();
                        let icon    = if is_dir { "📂" } else { "📄" };
                        let name    = f.name().to_string();
                        let orig    = human_size(f.size());
                        let comp    = human_size(f.compressed_size());
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {} ", icon), Style::default().fg(SH_FG)),
                            Span::styled(
                                format!("{:<50}", &name[..name.len().min(50)]),
                                Style::default().fg(if is_dir { SH_TYPE } else { SH_FG })
                            ),
                            Span::styled(format!(" {:>8} → {}", comp, orig), Style::default().fg(SH_CMT)),
                        ]));
                    }
                }
                if total > 150 {
                    lines.push(Line::from(Span::styled(
                        format!("  … {} more entries", total - 150),
                        Style::default().fg(SH_CMT)
                    )));
                }
                return PreviewContent::Highlighted(lines);
            }
            _ => {}
        }
    }

    let label = format!("📦  {} Archive", ext.to_uppercase());
    lines.push(Line::from(Span::styled(label, Style::default().fg(SH_FN))));
    lines.push(Line::from(Span::styled(
        "  Use 7-Zip or similar to inspect this archive.",
        Style::default().fg(SH_CMT)
    )));
    PreviewContent::Highlighted(lines)
}

// ── Text / code ───────────────────────────────────────────────────────────────

fn text_preview(p: &PathBuf) -> PreviewContent {
    let header = meta_header(p);
    let bytes = match fs::read(p) {
        Ok(b)  => b,
        Err(e) => {
            let mut lines = header;
            lines.push(Line::from(Span::styled(format!("⚠ {}", e), Style::default().fg(Color::Red))));
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
            Style::default().fg(SH_TYPE)
        )));
        lines.push(Line::from(Span::styled("─".repeat(70), Style::default().fg(SH_CMT))));
        lines.push(Line::from(vec![
            Span::styled("Offset    ", Style::default().fg(SH_CMT)),
            Span::styled("00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F  ", Style::default().fg(SH_OP)),
            Span::styled("ASCII", Style::default().fg(SH_STR)),
        ]));
        lines.extend(hex_lines);
        return PreviewContent::Highlighted(lines);
    }

    let text = String::from_utf8(bytes.clone())
        .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned());
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

    // Use syntax highlighting for code files
    if is_code(&ext) && !normalized.is_empty() {
        let mut lines = header;
        let total = normalized.lines().count();
        lines.extend(highlight_code(&normalized, &ext));
        if total > 400 {
            lines.push(Line::from(Span::styled(
                format!("  … {} more lines", total - 400),
                Style::default().fg(SH_CMT)
            )));
        }
        return PreviewContent::Highlighted(lines);
    }

    // Plain text
    let content: String = normalized.lines()
        .enumerate()
        .take(400)
        .map(|(i, line)| format!("{:>4}  {}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    let total = normalized.lines().count();
    let tail = if total > 400 {
        format!("\n\n… {} more lines", total - 400)
    } else { String::new() };

    // Convert meta_header to plain text prefix
    let header_text: String = header.iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    PreviewContent::Text(format!(
        "{}\n{}{}",
        header_text,
        if content.is_empty() { "<empty file>".to_string() } else { content },
        tail
    ))
}

// ── Hex dump lines ────────────────────────────────────────────────────────────

fn render_hex_lines(bytes: &[u8]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (chunk_i, chunk) in bytes[..bytes.len().min(512)].chunks(16).enumerate() {
        let offset = chunk_i * 16;
        let hex_part: String = chunk.chunks(8)
            .map(|g| g.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("  ");
        let ascii_part: String = chunk.iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '·' })
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
            Style::default().fg(SH_CMT)
        )));
    }
    lines
}

fn is_code(ext: &str) -> bool {
    matches!(ext,
        "rs"|"py"|"js"|"ts"|"tsx"|"jsx"|"html"|"htm"|"css"|"scss"|
        "json"|"toml"|"yaml"|"yml"|"sh"|"bash"|"zsh"|"ps1"|"bat"|
        "c"|"cpp"|"h"|"hpp"|"java"|"kt"|"go"|"rb"|"php"|"cs"|"lua"|
        "sql"|"xml"|"md"|"markdown"|"log"|"ini"|"cfg"|"conf"|
        "swift"|"dart"|"r"|"jl"|"hs"|"ex"|"exs"|"vue"|"svelte"|
        "astro"|"dockerfile"|"makefile"|"gitignore"|"env"|"txt"
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
            b'<' => { in_tag = true; tag.clear(); i += 1; }
            b'>' if in_tag => {
                in_tag = false;
                let t = tag.trim_start_matches('/');
                let name = t.split_whitespace().next().unwrap_or("");
                collect = matches!(name, "w:t"|"a:t"|"t"|"w:delText");
                if matches!(name, "w:p"|"/w:p"|"a:p"|"/a:p"|"w:br") { out.push('\n'); }
                i += 1;
            }
            _ if in_tag  => { tag.push(b[i] as char); i += 1; }
            _ if collect => { out.push(b[i] as char); i += 1; }
            _            => { i += 1; }
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
