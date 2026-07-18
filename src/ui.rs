// ================= src/ui.rs =================
use ratatui::{
    Frame,
    layout::{Layout, Direction, Constraint, Rect},
    widgets::{Block, Borders, BorderType, List, ListItem, Paragraph, Wrap, Clear},
    style::{Style, Color, Modifier},
    text::{Text, Line, Span},
};
use crate::state::{AppState, AppMode, DirLevel, LayoutGeometry};
use crate::preview::{self, PreviewContent};

// ─────────────────────────────────────────────────────────────────────────────
// Top-level draw
// ─────────────────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // breadcrumb
            Constraint::Min(0),    // panes
            Constraint::Length(1), // status bar
        ])
        .split(f.size());

    let mut geo = app.layout_geometry.lock();
    geo.breadcrumb_rect = chunks[0];
    geo.status_rect = chunks[2];
    geo.pane_rects.clear();
    geo.preview_rect = None;
    geo.row_rects.clear();
    geo.divider_rects.clear();

    draw_breadcrumb(f, app, chunks[0]);
    draw_panes(f, app, chunks[1], &mut geo);
    draw_status_bar(f, app, chunks[2]);
    
    if app.mode == AppMode::Rename || app.mode == AppMode::NewFolder {
        draw_input_modal(f, app);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Input Modal
// ─────────────────────────────────────────────────────────────────────────────

fn draw_input_modal(f: &mut Frame, app: &AppState) {
    let title = if app.mode == AppMode::Rename { " Rename " } else { " New Folder " };
    let term_size = f.size();
    
    let width = 60.min(term_size.width.saturating_sub(4));
    let height = 3;
    
    let area = Rect {
        x: term_size.x + (term_size.width.saturating_sub(width)) / 2,
        y: term_size.y + (term_size.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    
    f.render_widget(Clear, area);
    
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));
        
    let para = Paragraph::new(app.input_buffer.as_str())
        .block(block)
        .style(Style::default().fg(Color::White));
        
    f.render_widget(para, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Breadcrumb
// ─────────────────────────────────────────────────────────────────────────────

fn draw_breadcrumb(f: &mut Frame, app: &AppState, area: Rect) {
    let path = app.current().path.display().to_string();
    let label = if path == "\\\\drives" {
        " 💾  This PC — Drives".to_string()
    } else {
        format!(" 📁  {}", path)
    };
    let mode_badge = match app.mode {
        AppMode::Rename => "  ✏ RENAME  ",
        AppMode::ConfirmDelete => "  ❌ DELETE?  ",
        AppMode::NewFolder => "  📁 NEW FOLDER  ",
        _ => "",
    };

    let style = match app.mode {
        AppMode::Normal => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    };

    f.render_widget(
        Paragraph::new(format!("{}{}", label, mode_badge)).style(style),
        area,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-pane layout
// ─────────────────────────────────────────────────────────────────────────────

fn draw_panes(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    let num   = app.levels.len();
    let start = if num > 4 { num - 4 } else { 0 };
    let panes = &app.levels[start..];
    let np    = panes.len();

    let has_preview = panes.last().map_or(false, |l| !l.files.is_empty());

    let n_cols = if has_preview { np + 1 } else { np };

    // Get the normalized column ratios from AppState!
    let start_idx = 5 - n_cols;
    let mut sub_ratios = app.column_ratios[start_idx..].to_vec();
    let sum: f32 = sub_ratios.iter().sum();
    if sum > 0.0 {
        for r in sub_ratios.iter_mut() {
            *r /= sum;
        }
    }

    let mut constraints: Vec<Constraint> = sub_ratios.iter().take(n_cols.saturating_sub(1)).map(|&r| {
        Constraint::Percentage((r * 100.0) as u16)
    }).collect();
    if n_cols > 0 {
        constraints.push(Constraint::Min(0));
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);
        
    for i in 0..chunks.len().saturating_sub(1) {
        let current_chunk = chunks[i];
        if current_chunk.width > 0 {
            let next_chunk = chunks[i+1];
            let divider_x = if current_chunk.x + current_chunk.width == next_chunk.x {
                current_chunk.x + current_chunk.width - 1
            } else {
                next_chunk.x.saturating_sub(1)
            };
            geo.divider_rects.push(Rect {
                x: divider_x,
                y: current_chunk.y,
                width: 2, // slightly larger hit area
                height: current_chunk.height,
            });
        }
    }

    for (i, level) in panes.iter().enumerate() {
        geo.pane_rects.push(chunks[i]);
        draw_dir_pane(f, level, (start + i) == app.current_level, chunks[i], geo, (start + i) - start);
    }

    if has_preview {
        if let Some(current) = panes.last() {
            geo.preview_rect = Some(chunks[np]);
            draw_preview_pane(f, app, current, chunks[np]);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Directory column pane
// ─────────────────────────────────────────────────────────────────────────────

fn draw_dir_pane(f: &mut Frame, level: &DirLevel, is_current: bool, area: Rect, geo: &mut LayoutGeometry, pane_idx: usize) {
    let path_str = level.path.display().to_string();
    let title = if path_str == "\\\\drives" {
        "Drives".to_string()
    } else {
        level.path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.trim_end_matches(['/', '\\']).to_string())
    };

    let border_style = if is_current {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let visible_h    = area.height.saturating_sub(2) as usize;
    let scroll_start = if is_current && level.selected >= visible_h {
        level.selected - visible_h + 1
    } else { 0 };

    let mut row_rects_for_pane = Vec::new();
    let list_inner_area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let items: Vec<ListItem> = level.files
        .iter()
        .enumerate()
        .skip(scroll_start)
        .take(visible_h)
        .enumerate()
        .map(|(render_i, (file_i, p))| {
            // Save row rect for hit testing
            row_rects_for_pane.push((file_i, Rect {
                x: list_inner_area.x,
                y: list_inner_area.y + render_i as u16,
                width: list_inner_area.width,
                height: 1,
            }));
            
            let raw_name = p.path.file_name()
                .unwrap_or_else(|| p.path.as_os_str())
                .to_string_lossy()
                .to_string();

            let display = if p.is_dir {
                format!("📂 {}", raw_name)
            } else {
                let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                format!("{} {}", file_icon(ext), raw_name)
            };

            let is_selected = file_i == level.selected;
            if is_selected {
                let color = if is_current { Color::Cyan } else { Color::Rgb(88, 91, 112) };
                let fg_color = if is_current { Color::Black } else { Color::White };
                
                let display_width = if p.is_dir {
                    2 + 1 + raw_name.chars().count() // "📂" (2) + " " (1) + name
                } else {
                    let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    let icon = file_icon(ext);
                    let icon_w = if icon.is_empty() { 0 } else { 2 };
                    icon_w + 1 + raw_name.chars().count()
                };

                let inner_w = (area.width as usize).saturating_sub(2);
                let pad_w = (inner_w.saturating_sub(3)).saturating_sub(display_width);
                let padded_text = format!(" {}{}", display, " ".repeat(pad_w));

                let spans = vec![
                    Span::styled("", Style::default().fg(color)),
                    Span::styled(padded_text, Style::default().fg(fg_color).bg(color).add_modifier(Modifier::BOLD)),
                    Span::styled("", Style::default().fg(color)),
                ];
                ListItem::new(Line::from(spans))
            } else {
                let style = if is_current {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                ListItem::new(format!("  {}", display)).style(style)
            }
        })
        .collect();

    geo.row_rects.insert(pane_idx, row_rects_for_pane);

    let list = List::new(items).block(
        Block::default()
            .title(format!(" {} ", if title.is_empty() { "/" } else { &title }))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style),
    );
    f.render_widget(list, area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Preview pane
// ─────────────────────────────────────────────────────────────────────────────

fn draw_preview_pane(f: &mut Frame, app: &AppState, level: &DirLevel, area: Rect) {
    if level.files.is_empty() {
        app.native_preview.hide();
        f.render_widget(
            Paragraph::new("Empty")
                .block(Block::default().title(" Preview ").borders(Borders::ALL).border_type(BorderType::Rounded))
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let selected = &level.files[level.selected];

    // ── Directory preview ─────────────────────────────────────────────────────
    if selected.is_dir {
        let children   = preview::cached_list_dir(&selected.path);
        let dir_count  = children.iter().filter(|p| p.is_dir).count();
        let file_count = children.len() - dir_count;
        let img_count  = children.iter().filter(|p| {
            let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            matches!(ext.as_str(), "jpg"|"jpeg"|"png"|"gif"|"bmp"|"webp"|"tiff"|"tif")
        }).count();

        // Put stats in the title bar — compact, not taking content space
        let dir_name = selected.path.file_name()
            .unwrap_or_default().to_string_lossy().to_string();
        let stats_title = if img_count > 0 {
            format!(" {} │ {} dirs  {} files  {} imgs ", dir_name, dir_count, file_count, img_count)
        } else {
            format!(" {} │ {} dirs  {} files ", dir_name, dir_count, file_count)
        };

        let visible_h = area.height.saturating_sub(2) as usize;

        let items: Vec<ListItem> = children.iter()
            .take(visible_h)
            .map(|p| {
                let name = p.path.file_name()
                    .unwrap_or_else(|| p.path.as_os_str())
                    .to_string_lossy()
                    .to_string();
                let (icon, color) = if p.is_dir {
                    ("📂 ", Color::White)
                } else {
                    let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    (file_icon(ext), Color::Rgb(180, 185, 200))
                };
                ListItem::new(format!("  {}{}", icon, name))
                    .style(Style::default().fg(color))
            })
            .collect();

        f.render_widget(
            List::new(items).block(
                Block::default()
                    .title(stats_title)
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Green)),
            ),
            area,
        );
        app.native_preview.hide();
        return;
    }

    // ── File preview ──────────────────────────────────────────────────────────
    let is_image = {
        let ext = selected.path.extension()
            .and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        matches!(ext.as_str(), "jpg"|"jpeg"|"png"|"bmp"|"gif"|"webp"|"tiff"|"tif"|"ico")
    };

    let name = selected.path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = std::fs::metadata(&selected.path);
    let size_str = meta.as_ref()
        .map(|m| preview::human_size(m.len()))
        .unwrap_or_default();
    let ext = selected.path.extension().and_then(|s| s.to_str()).unwrap_or("").to_uppercase();

    let title = if app.mode != AppMode::Normal {
        format!(" Preview │ Mode active (Esc to cancel) ")
    } else {
        " PREVIEW ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.mode != AppMode::Normal {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray) // Neutral gray for borders
        });

    let inner = block.inner(area);
    f.render_widget(block, area);

    let scroll = app.preview_scroll as u16;

    match preview::render(&selected.path, app.image_rotation, app.image_flip_h) {
        PreviewContent::Text(txt) => {
            app.native_preview.hide();
            let para = Paragraph::new(txt)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::Highlighted(lines) => {
            app.native_preview.hide();
            let text = Text::from(lines);
            let para = Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::ImageFallback(info) => {
            let mut top_margin = 0;
            if let Some(img) = &info.img {
                let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
                // We need to offset the image downwards by 4 cells so it doesn't cover the header
                let mut img_rect = inner;
                if img_rect.height > 6 {
                    img_rect.y += 4;
                    img_rect.height -= 4;
                }
                app.native_preview.show(std::sync::Arc::clone(img), img_rect, cols, rows);
                top_margin = img_rect.y - inner.y;
            } else {
                app.native_preview.hide();
            }

            let mut text = vec![
                Line::from(vec![Span::styled(&name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]),
            ];

            let mut meta_str = format!("{} Image  •  {}", ext, size_str);
            if let Some((w, h)) = info.dimensions {
                meta_str.push_str(&format!("  •  {} × {}", w, h));
            }
            text.push(Line::from(vec![Span::styled(meta_str, Style::default().fg(Color::DarkGray))]));
            text.push(Line::from(vec![Span::styled("────────────────────────────────────────────", Style::default().fg(Color::DarkGray))]));
            
            // Add padding for image
            for _ in 0..top_margin.saturating_sub(text.len() as u16) {
                text.push(Line::from(""));
            }

            let para = Paragraph::new(Text::from(text))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// File icons
// ─────────────────────────────────────────────────────────────────────────────

fn file_icon(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "rs"                                              => "🦀",
        "py"                                              => "🐍",
        "js"|"mjs"|"cjs"                                  => "📜",
        "ts"|"tsx"|"jsx"                                  => "📘",
        "html"|"htm"                                      => "🌐",
        "css"|"scss"|"sass"|"less"                        => "🎨",
        "json"|"toml"|"yaml"|"yml"                        => "⚙️ ",
        "sh"|"bash"|"zsh"|"ps1"|"bat"|"cmd"               => "⚡",
        "c"|"cpp"|"h"|"hpp"|"cc"                          => "🔧",
        "java"|"kt"|"kts"                                 => "☕",
        "go"                                              => "🐹",
        "rb"                                              => "💎",
        "php"                                             => "🐘",
        "swift"                                           => "🕊️ ",
        "cs"                                              => "🔷",
        "lua"                                             => "🌙",
        "sql"                                             => "🗄️ ",
        "txt"|"log"                                       => "📝",
        "md"|"markdown"|"rst"                             => "📖",
        "pdf"                                             => "📄",
        "docx"|"doc"|"odt"                                => "📘",
        "xlsx"|"xls"|"ods"|"csv"|"tsv"                    => "📊",
        "pptx"|"ppt"|"odp"                                => "📊",
        "ipynb"                                            => "📓",
        "rtf"                                              => "📄",
        "zip"|"7z"|"rar"                                  => "📦",
        "tar"|"gz"|"bz2"|"xz"|"zst"|"tgz"                => "📦",
        "iso"|"img"                                       => "💿",
        "jpg"|"jpeg"|"png"|"gif"|"bmp"|"webp"|"tiff"|"ico" => "🖼️ ",
        "svg"                                             => "🖼️ ",
        "mp4"|"mkv"|"avi"|"mov"|"webm"|"flv"|"wmv"       => "🎬",
        "mp3"|"flac"|"wav"|"ogg"|"aac"|"m4a"|"opus"      => "🎵",
        "exe"|"msi"                                       => "🖥️ ",
        "dll"|"so"|"dylib"                                => "🔩",
        "apk"|"ipa"                                       => "📱",
        "ttf"|"otf"|"woff"|"woff2"                        => "🔤",
        _                                                 => "📄",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Status bar
// ─────────────────────────────────────────────────────────────────────────────

fn draw_status_bar(f: &mut Frame, app: &AppState, area: Rect) {
    let cur   = app.current();
    let count = cur.files.len();
    let pos   = if count > 0 { cur.selected + 1 } else { 0 };

    let text = match app.mode {
        AppMode::Rename => format!(" {}/{} │ ✏ RENAME: type new name · Enter confirm · Esc cancel", pos, count.max(1)),
        AppMode::ConfirmDelete => format!(" {}/{} │ ❌ DELETE: Y confirm · Esc cancel", pos, count.max(1)),
        AppMode::NewFolder => format!(" {}/{} │ 📁 NEW FOLDER: type name · Enter confirm · Esc cancel", pos, count.max(1)),
        _ => format!(
            " {}/{} │ q:quit  ←↓↑→/hjkl  g/G:top/bot  ~:home  \\:drives  [/]/[?]:scroll  F2:rename  Del:delete",
            pos, count.max(1)
        )
    };

    let style = if app.mode != AppMode::Normal {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::Black).bg(Color::Gray)
    };

    f.render_widget(Paragraph::new(text).style(style), area);
}
