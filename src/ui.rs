// ================= src/ui.rs =================
use ratatui::{
    Frame,
    layout::{Layout, Direction, Constraint, Rect},
    widgets::{Block, Borders, BorderType, List, ListItem, Paragraph, Wrap, Clear},
    style::{Style, Color, Modifier},
    text::{Text, Line, Span},
};
use crate::state::{AppState, AppMode, DirLevel, LayoutGeometry, ContextAction};
use crate::preview::{self, PreviewContent};

// ── Theme ────────────────────────────────────────────────────────────────────
const C_ACCENT:   Color = Color::Rgb(65, 166, 166);   // calm, softer cyan focus accent
const C_ACCENT2:  Color = Color::Rgb(137, 180, 250);  // soft blue
const C_BG_PANEL: Color = Color::Rgb(24, 26, 34);
const C_BORDER:   Color = Color::Rgb(38, 40, 50);    // subtle border
const C_BORDER_LO:Color = Color::Rgb(28, 30, 37);    // blends into panel bg
const C_TEXT:     Color = Color::Rgb(214, 218, 230);
const C_TEXT_SOFT:Color = Color::Rgb(160, 166, 185);  // softer grey for preview body text
const C_MUTED:    Color = Color::Rgb(120, 126, 145);
const C_WARN:     Color = Color::Rgb(240, 198, 116);
const C_OK:       Color = Color::Rgb(137, 220, 165);
const C_ERR:      Color = Color::Rgb(240, 120, 120);
const C_SEL_BG:   Color = Color::Rgb(32, 64, 66);    // soft background selection highlight
const C_SEL_BG_INACTIVE: Color = Color::Rgb(40, 42, 52);
const C_MARK:     Color = Color::Rgb(210, 160, 90);
const C_FOLDER:   Color = Color::Rgb(229, 192, 123);  // VS Code yellow/gold folder

// ─────────────────────────────────────────────────────────────────────────────
// Top-level draw
// ─────────────────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // sidebar + content
            Constraint::Length(1), // status bar (hints live here — no more duplicate top bar)
        ])
        .split(f.size());

    let mut geo = app.layout_geometry.lock();
    geo.status_rect = root[1];
    geo.pane_rects.clear();
    geo.preview_rect = None;
    geo.row_rects.clear();
    geo.divider_rects.clear();
    geo.sidebar_item_rects.clear();
    geo.breadcrumb_segment_rects.clear();
    geo.context_menu_item_rects.clear();
    geo.context_menu_rect = None;

    // Responsive: hide the sidebar on very narrow terminals so panes stay usable.
    let show_sidebar = root[0].width >= 60;
    let sidebar_w = if show_sidebar { app.sidebar_width.min(root[0].width.saturating_sub(30)) } else { 0 };

    let body = if show_sidebar {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(sidebar_w), Constraint::Length(1), Constraint::Min(0)])
            .split(root[0])
    } else {
        Layout::default().constraints([Constraint::Min(0)]).split(root[0])
    };

    if show_sidebar {
        geo.sidebar_rect = body[0];
        geo.sidebar_divider_rect = body[1];
        draw_sidebar(f, app, body[0], &mut geo);
        draw_vertical_divider(f, body[1]);
    } else {
        geo.sidebar_rect = Rect::default();
        geo.sidebar_divider_rect = Rect::default();
    }

    let main_area = if show_sidebar { body[2] } else { body[0] };
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(main_area);

    geo.breadcrumb_rect = main[0];
    draw_breadcrumb(f, app, main[0], &mut geo);
    draw_panes(f, app, main[1], &mut geo);
    draw_status_bar(f, app, root[1]);

    if app.mode == AppMode::Rename || app.mode == AppMode::NewFolder {
        draw_input_modal(f, app);
    }
    if app.mode == AppMode::ConfirmDelete || app.mode == AppMode::ConfirmDeletePermanent {
        draw_confirm_modal(f, app);
    }
    if app.mode == AppMode::Properties {
        draw_properties_modal(f, app);
    }
    if app.mode == AppMode::ContextMenu {
        draw_context_menu(f, app, &mut geo);
    }
}

fn draw_vertical_divider(f: &mut Frame, area: Rect) {
    for y in area.y..area.y + area.height {
        f.render_widget(
            Paragraph::new(Span::styled("│", Style::default().fg(C_BORDER_LO))),
            Rect { x: area.x, y, width: 1, height: 1 },
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Left sidebar: Quick Access + Drives (click to navigate)
// ─────────────────────────────────────────────────────────────────────────────

fn draw_sidebar(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    let block = Block::default()
        .title(" DRIVES & LOCATIONS ")
        .title_style(Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_BORDER_LO))
        .style(Style::default().bg(C_BG_PANEL));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cur_path = &app.current().path;
    let mut y = inner.y;
    let max_y = inner.y + inner.height;

    // Top padding
    y += 1;

    push_line(f, geo, inner, &mut y, max_y,
        Line::from(Span::styled("  QUICK ACCESS", Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD))), None);
    
    let set = get_icon_set();
    for (label, path) in app.quick_access.iter() {
        let is_active = *path == *cur_path;
        let text_style = if is_active {
            Style::default().fg(Color::Black).bg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_TEXT)
        };
        let label_lower = label.to_lowercase();
        let icon = if label_lower.contains("home") {
            set.home
        } else if label_lower.contains("desktop") {
            set.desktop
        } else if label_lower.contains("document") {
            set.documents
        } else if label_lower.contains("download") {
            set.downloads
        } else if label_lower.contains("picture") {
            set.pictures
        } else {
            " "
        };
        let clean_label_str = label.chars()
            .skip_while(|c| !c.is_alphabetic())
            .collect::<String>();
        let clean_label = if clean_label_str.is_empty() { *label } else { &clean_label_str };
        
        let icon_span = Span::styled(icon, if is_active { text_style } else { Style::default().fg(C_ACCENT2) });
        let gap_w = 4_usize.saturating_sub(icon_span.width());
        let text_span = Span::styled(format!("{}{}", " ".repeat(gap_w), clean_label), text_style);
        
        push_line(f, geo, inner, &mut y, max_y,
            Line::from(vec![
                Span::styled("  ", text_style),
                icon_span,
                text_span,
            ]),
            Some(path.clone()));
    }

    // Increased spacing between sections
    y += 2;
    push_line(f, geo, inner, &mut y, max_y,
        Line::from(Span::styled("  DRIVES", Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD))), None);

    for d in app.drives.iter() {
        if y + 2 >= max_y { break; }
        let is_active = d.path == *cur_path;
        let dot_color = match d.kind.as_str() {
            "Removable" => C_WARN,
            "Network"   => C_OK,
            "CD-ROM"    => C_MUTED,
            _           => C_ACCENT2,
        };
        let letter_color = if is_active { C_ACCENT } else { dot_color };
        let letter = d.path.to_string_lossy().trim_end_matches('\\').to_string();
        
        let icon_span = Span::styled(set.drive, Style::default().fg(letter_color));
        let text_span = Span::styled(format!("  {}  ", letter), Style::default().fg(letter_color).add_modifier(Modifier::BOLD));
        
        push_line(f, geo, inner, &mut y, max_y,
            Line::from(vec![
                Span::styled("  ", Style::default()),
                icon_span,
                text_span,
                Span::styled(truncate(&d.label, inner.width.saturating_sub(13) as usize), Style::default().fg(C_MUTED)),
            ]),
            Some(d.path.clone()));

        if d.total > 0 {
            let used = d.total.saturating_sub(d.free);
            let frac = (used as f64 / d.total as f64).clamp(0.0, 1.0);
            let bar_w = inner.width.saturating_sub(4) as usize;
            let filled = ((bar_w as f64) * frac).round() as usize;
            let bar_color = if frac > 0.9 { C_ERR } else if frac > 0.75 { C_WARN } else { C_ACCENT2 };
            let mut bar = String::new();
            bar.push_str(&"█".repeat(filled.min(bar_w)));
            bar.push_str(&"░".repeat(bar_w.saturating_sub(filled)));
            let free_gb = d.free as f64 / 1_073_741_824.0;
            let total_gb = d.total as f64 / 1_073_741_824.0;
            push_line(f, geo, inner, &mut y, max_y,
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(format!("{}", bar), Style::default().fg(bar_color)),
                ]),
                None);
            push_line(f, geo, inner, &mut y, max_y,
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(format!("{:.0} GB free of {:.0} GB", free_gb, total_gb), Style::default().fg(C_MUTED)),
                ]),
                None);
        }
    }
}

/// Renders one sidebar row at the current `y` cursor, advances it, and
/// (optionally) registers a click-navigation hitbox for it.
fn push_line(
    f: &mut Frame,
    geo: &mut LayoutGeometry,
    inner: Rect,
    y: &mut u16,
    max_y: u16,
    content: Line<'static>,
    click_path: Option<std::path::PathBuf>,
) {
    if *y >= max_y { return; }
    let rect = Rect { x: inner.x, y: *y, width: inner.width, height: 1 };
    f.render_widget(Paragraph::new(content), rect);
    if let Some(p) = click_path {
        geo.sidebar_item_rects.push((rect, p));
    }
    *y += 1;
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    if max <= 3 { return ".".repeat(max.max(1)); }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Notifications / non-blocking toasts
// ─────────────────────────────────────────────────────────────────────────────

// (rendered inline in the status bar — see draw_status_bar)

// ─────────────────────────────────────────────────────────────────────────────
// Input Modal (Rename / New Folder) — shows cursor + selection
// ─────────────────────────────────────────────────────────────────────────────

fn draw_input_modal(f: &mut Frame, app: &AppState) {
    let title = if app.mode == AppMode::Rename { " Rename " } else { " New Folder " };
    let term_size = f.size();

    let width = 64.min(term_size.width.saturating_sub(4)).max(20);
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
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(Color::Rgb(28, 30, 38)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chars: Vec<char> = app.input_buffer.chars().collect();
    let (lo, hi) = if app.input_sel_start <= app.input_cursor {
        (app.input_sel_start, app.input_cursor)
    } else {
        (app.input_cursor, app.input_sel_start)
    };

    let mut spans = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        let selected = i >= lo && i < hi;
        let style = if selected {
            Style::default().fg(Color::Black).bg(C_ACCENT2)
        } else {
            Style::default().fg(C_TEXT)
        };
        spans.push(Span::styled(c.to_string(), style));
    }
    // Cursor caret (only when there's no active selection to show).
    if lo == hi {
        let cursor_pos = app.input_cursor.min(spans.len());
        spans.insert(cursor_pos, Span::styled("│", Style::default().fg(C_ACCENT).add_modifier(Modifier::RAPID_BLINK)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_confirm_modal(f: &mut Frame, app: &AppState) {
    let permanent = app.mode == AppMode::ConfirmDeletePermanent;
    let n = app.selected_paths().len().max(1);
    let title = if permanent { " Permanently Delete " } else { " Delete " };
    let msg = if permanent {
        format!("Permanently delete {} item(s)? This cannot be undone.", n)
    } else {
        format!("Delete {} item(s)?", n)
    };

    let term_size = f.size();
    let width = 56.min(term_size.width.saturating_sub(4)).max(20);
    let height = 5;
    let area = Rect {
        x: term_size.x + (term_size.width.saturating_sub(width)) / 2,
        y: term_size.y + (term_size.height.saturating_sub(height)) / 2,
        width, height,
    };
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ERR))
        .style(Style::default().bg(Color::Rgb(28, 30, 38)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let text = vec![
        Line::from(Span::styled(msg, Style::default().fg(C_TEXT))),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Y ", Style::default().fg(Color::Black).bg(C_ERR)),
            Span::raw(" confirm     "),
            Span::styled(" Esc ", Style::default().fg(Color::Black).bg(C_MUTED)),
            Span::raw(" cancel"),
        ]),
    ];
    f.render_widget(Paragraph::new(text), inner);
}

fn draw_properties_modal(f: &mut Frame, app: &AppState) {
    let Some(path) = app.selected_file().or_else(|| {
        let cur = app.current();
        if cur.files.is_empty() { None } else { Some(cur.files[cur.selected].path.clone()) }
    }) else { return; };

    let term_size = f.size();
    let width = 62.min(term_size.width.saturating_sub(4)).max(30);
    let height = 12.min(term_size.height.saturating_sub(4)).max(8);
    let area = Rect {
        x: term_size.x + (term_size.width.saturating_sub(width)) / 2,
        y: term_size.y + (term_size.height.saturating_sub(height)) / 2,
        width, height,
    };
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(" Properties ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(Color::Rgb(28, 30, 38)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let meta = std::fs::metadata(&path);
    let mut lines = vec![
        Line::from(vec![Span::styled("Name:      ", Style::default().fg(C_MUTED)), Span::styled(name, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::styled("Location:  ", Style::default().fg(C_MUTED)), Span::styled(path.parent().map(|p| p.display().to_string()).unwrap_or_default(), Style::default().fg(C_TEXT))]),
    ];
    match meta {
        Ok(m) => {
            let kind = if m.is_dir() { "Folder".to_string() } else { "File".to_string() };
            lines.push(Line::from(vec![Span::styled("Type:      ", Style::default().fg(C_MUTED)), Span::styled(kind, Style::default().fg(C_TEXT))]));
            if !m.is_dir() {
                lines.push(Line::from(vec![Span::styled("Size:      ", Style::default().fg(C_MUTED)), Span::styled(preview::human_size(m.len()), Style::default().fg(C_TEXT))]));
            }
            if let Ok(modified) = m.modified() {
                if let Ok(dt) = modified.duration_since(std::time::UNIX_EPOCH) {
                    lines.push(Line::from(vec![Span::styled("Modified:  ", Style::default().fg(C_MUTED)), Span::styled(format_epoch(dt.as_secs()), Style::default().fg(C_TEXT))]));
                }
            }
            let mut attrs = Vec::new();
            if m.permissions().readonly() { attrs.push("Read-only"); }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                let a = m.file_attributes();
                if a & 0x2 != 0 { attrs.push("Hidden"); }
                if a & 0x4 != 0 { attrs.push("System"); }
            }
            if !attrs.is_empty() {
                lines.push(Line::from(vec![Span::styled("Attributes:", Style::default().fg(C_MUTED)), Span::styled(format!(" {}", attrs.join(", ")), Style::default().fg(C_WARN))]));
            }
        }
        Err(e) => {
            lines.push(Line::from(vec![Span::styled("Error:     ", Style::default().fg(C_ERR)), Span::styled(e.to_string(), Style::default().fg(C_ERR))]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Esc to close", Style::default().fg(C_MUTED))));

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn format_epoch(secs: u64) -> String {
    // Lightweight, dependency-free UTC formatting (no timezone database needed).
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Days since 1970-01-01 -> Y-M-D (civil_from_days algorithm).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mth <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, mth, d, h, m, s)
}

// ─────────────────────────────────────────────────────────────────────────────
// Right-click context menu
// ─────────────────────────────────────────────────────────────────────────────

fn draw_context_menu(f: &mut Frame, app: &AppState, geo: &mut LayoutGeometry) {
    let items = &app.context_menu_items;
    if items.is_empty() { return; }

    let term = f.size();
    let width: u16 = items.iter().map(|a| a.label().len() as u16).max().unwrap_or(10) + 6;
    let height = items.len() as u16 + 2;

    let (mx, my) = app.pending_menu_pos;
    let mut x = mx;
    let mut y = my.saturating_add(1);
    if x + width > term.width { x = term.width.saturating_sub(width); }
    if y + height > term.height { y = term.height.saturating_sub(height); }

    let area = Rect { x, y, width: width.min(term.width), height: height.min(term.height) };
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(Color::Rgb(30, 32, 40)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    for (i, action) in items.iter().enumerate() {
        if i as u16 >= inner.height { break; }
        let rect = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let is_hovered = app.context_menu_hover == Some(i);

        let base_fg = if matches!(action, ContextAction::Delete) { C_ERR } else { C_TEXT };
        let (fg, bg) = if is_hovered {
            (Color::Black, C_ACCENT)
        } else {
            (base_fg, Color::Rgb(30, 32, 40))
        };

        let label_text = format!(" {}", action.label());
        let pad = " ".repeat((inner.width as usize).saturating_sub(label_text.chars().count()));
        let style = if is_hovered {
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg).bg(bg)
        };
        f.render_widget(Paragraph::new(Line::from(Span::styled(format!("{}{}", label_text, pad), style))), rect);
        geo.context_menu_item_rects.push((rect, *action));
    }
    geo.context_menu_rect = Some(area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Breadcrumb — clickable path segments
// ─────────────────────────────────────────────────────────────────────────────

fn draw_breadcrumb(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    let path = app.current().path.clone();
    let path_str = path.display().to_string();

    if path_str == "\\\\drives" {
        f.render_widget(
            Paragraph::new(Span::styled(" This PC — Drives", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))),
            area,
        );
        return;
    }

    // Build clickable segments: C:\ > Users > sifay > Pictures
    let mut spans = vec![Span::styled("  ", Style::default().fg(C_MUTED))];
    let mut segments: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut acc = std::path::PathBuf::new();
    for comp in path.components() {
        acc.push(comp.as_os_str());
        let label = comp.as_os_str().to_string_lossy().to_string();
        segments.push((label, acc.clone()));
    }

    let mut x = area.x + 2;
    for (i, (label, seg_path)) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let style = if is_last {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_TEXT)
        };
        let text = label.trim_end_matches(['\\', '/']).to_string();
        let text = if text.is_empty() { label.clone() } else { text };
        let w = text.chars().count() as u16;
        if x + w <= area.x + area.width {
            geo.breadcrumb_segment_rects.push((Rect { x, y: area.y, width: w, height: 1 }, seg_path.clone()));
        }
        spans.push(Span::styled(text, style));
        x += w;
        if !is_last {
            spans.push(Span::styled("    >    ", Style::default().fg(C_MUTED)));
            x += 9;
        }
    }

    let mode_badge = match app.mode {
        AppMode::Rename => "  [RENAME]",
        AppMode::ConfirmDelete | AppMode::ConfirmDeletePermanent => "  [DELETE?]",
        AppMode::NewFolder => "  [NEW FOLDER]",
        AppMode::ContextMenu => "  [MENU]",
        AppMode::Properties => "  [PROPERTIES]",
        _ => "",
    };
    if !mode_badge.is_empty() {
        spans.push(Span::styled(mode_badge, Style::default().fg(C_WARN).add_modifier(Modifier::BOLD)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
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

    if n_cols == 0 { return; }

    let start_idx = 5 - n_cols;
    let mut sub_ratios = app.column_ratios[start_idx..].to_vec();
    let sum: f32 = sub_ratios.iter().sum();
    if sum > 0.0 {
        for r in sub_ratios.iter_mut() { *r /= sum; }
    }

    let mut constraints: Vec<Constraint> = sub_ratios.iter().take(n_cols.saturating_sub(1)).map(|&r| {
        Constraint::Percentage((r * 100.0) as u16)
    }).collect();
    constraints.push(Constraint::Min(0));

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    // Divider hitboxes — kept narrow (1 cell) so they don't swallow clicks on
    // filenames near a column edge, but still easy to grab precisely.
    for i in 0..chunks.len().saturating_sub(1) {
        let current_chunk = chunks[i];
        if current_chunk.width > 0 {
            let next_chunk = chunks[i + 1];
            let divider_x = if current_chunk.x + current_chunk.width == next_chunk.x {
                current_chunk.x + current_chunk.width - 1
            } else {
                next_chunk.x.saturating_sub(1)
            };
            geo.divider_rects.push(Rect { x: divider_x, y: current_chunk.y, width: 1, height: current_chunk.height });
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
        Style::default().fg(C_ACCENT)
    } else {
        Style::default().fg(C_BORDER)
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
    let name_budget = (list_inner_area.width as usize).saturating_sub(4);

    let items: Vec<ListItem> = level.files
        .iter()
        .enumerate()
        .skip(scroll_start)
        .take(visible_h)
        .enumerate()
        .map(|(render_i, (file_i, p))| {
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
            let shown_name = truncate(&raw_name, name_budget.max(4));

            let folder_icon = get_icon_set().folder;
            let (icon, icon_color) = if p.is_dir {
                (folder_icon, C_FOLDER)
            } else {
                let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                file_icon(ext)
            };
            let is_marked = level.marked.contains(&p.path);
            let is_selected = file_i == level.selected;

            let icon_span = Span::styled(icon, Style::default().fg(icon_color));

            if is_selected {
                let bg = if is_current { C_SEL_BG } else { C_SEL_BG_INACTIVE };
                let fg_color = if is_current { C_ACCENT } else { C_TEXT };
                let inner_w = list_inner_area.width as usize;
                
                let icon_selected_span = Span::styled(icon, Style::default().fg(icon_color).bg(bg));
                let name_selected_span = Span::styled(shown_name.clone(), Style::default().fg(fg_color).bg(bg).add_modifier(Modifier::BOLD));
                
                let disp_w = icon_selected_span.width() + 1 + name_selected_span.width();
                let pad_w = inner_w.saturating_sub(disp_w);
                
                ListItem::new(Line::from(vec![
                    icon_selected_span,
                    Span::styled(" ", Style::default().bg(bg)),
                    name_selected_span,
                    Span::styled(" ".repeat(pad_w), Style::default().bg(bg)),
                ]))
            } else if is_marked {
                ListItem::new(Line::from(vec![
                    icon_span,
                    Span::styled(" ", Style::default()),
                    Span::styled(shown_name.clone(), Style::default().fg(C_MARK).add_modifier(Modifier::BOLD)),
                ]))
            } else {
                let style = if is_current { Style::default().fg(C_TEXT) } else { Style::default().fg(C_MUTED) };
                ListItem::new(Line::from(vec![
                    icon_span,
                    Span::styled(" ", Style::default()),
                    Span::styled(shown_name.clone(), style),
                ]))
            }
        })
        .collect();

    geo.row_rects.insert(pane_idx, row_rects_for_pane);

    let marked_count = level.marked.len();
    let title_text = if marked_count > 0 {
        format!(" {} · {} marked ", if title.is_empty() { "/" } else { &title }, marked_count)
    } else {
        format!(" {} ", if title.is_empty() { "/" } else { &title })
    };

    let list = List::new(items).block(
        Block::default()
            .title(title_text)
            .title_style(if is_current { Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD) } else { Style::default().fg(C_MUTED) })
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style),
    );
    f.render_widget(list, area);

    // Simple scrollbar indicator for long directories.
    if level.files.len() > visible_h && area.height > 4 {
        let track_h = area.height.saturating_sub(2) as usize;
        let ratio = visible_h as f64 / level.files.len() as f64;
        let thumb_h = ((track_h as f64) * ratio).max(1.0) as usize;
        let thumb_pos = if level.files.len() > visible_h {
            ((track_h.saturating_sub(thumb_h)) as f64 * (scroll_start as f64 / (level.files.len() - visible_h) as f64)) as usize
        } else { 0 };
        for i in 0..track_h {
            let on_thumb = i >= thumb_pos && i < thumb_pos + thumb_h;
            let ch = if on_thumb { "█" } else { "│" };
            let color = if on_thumb { C_ACCENT2 } else { C_BORDER };
            let rect = Rect { x: area.x + area.width - 1, y: area.y + 1 + i as u16, width: 1, height: 1 };
            f.render_widget(Paragraph::new(Span::styled(ch, Style::default().fg(color))), rect);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preview pane
// ─────────────────────────────────────────────────────────────────────────────

fn draw_preview_pane(f: &mut Frame, app: &AppState, level: &DirLevel, area: Rect) {
    if level.files.is_empty() {
        app.native_preview.hide();
        f.render_widget(
            Paragraph::new(Span::styled("Empty", Style::default().fg(C_MUTED)))
                .block(Block::default().title(" PREVIEW ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(C_BORDER))),
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

        let dir_name = selected.path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let stats_title = if img_count > 0 {
            format!(" {} │ {} dirs  {} files  {} imgs ", dir_name, dir_count, file_count, img_count)
        } else {
            format!(" {} │ {} dirs  {} files ", dir_name, dir_count, file_count)
        };

        let visible_h = area.height.saturating_sub(2) as usize;
        let name_budget = (area.width as usize).saturating_sub(6);

        let items: Vec<ListItem> = children.iter()
            .take(visible_h)
            .map(|p| {
                let raw = p.path.file_name().unwrap_or_else(|| p.path.as_os_str()).to_string_lossy();
                let name = truncate(&raw, name_budget.max(4));
                let folder_icon = get_icon_set().folder;
                let (icon, icon_color) = if p.is_dir {
                    (folder_icon, C_FOLDER)
                } else {
                    let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    file_icon(ext)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(icon, Style::default().fg(icon_color)),
                    Span::styled(" ", Style::default()),
                    Span::styled(name, Style::default().fg(C_TEXT_SOFT)),
                ]))
            })
            .collect();

        f.render_widget(
            List::new(items).block(
                Block::default()
                    .title(stats_title)
                    .title_style(Style::default().fg(C_OK))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(C_BORDER_LO)),
            ),
            area,
        );
        app.native_preview.hide();
        return;
    }

    // ── File preview ──────────────────────────────────────────────────────────
    let name = selected.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let meta = std::fs::metadata(&selected.path);
    let size_str = meta.as_ref().map(|m| preview::human_size(m.len())).unwrap_or_default();
    let ext = selected.path.extension().and_then(|s| s.to_str()).unwrap_or("").to_uppercase();

    let title = if app.mode != AppMode::Normal {
        " PREVIEW │ mode active (Esc to cancel) ".to_string()
    } else {
        " PREVIEW ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(if app.mode != AppMode::Normal { Style::default().fg(C_WARN) } else { Style::default().fg(C_MUTED) })
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.mode != AppMode::Normal { Style::default().fg(C_WARN) } else { Style::default().fg(C_BORDER_LO) });

    let inner = block.inner(area);
    f.render_widget(block, area);

    let scroll = app.preview_scroll as u16;

    match preview::render(&selected.path, app.image_rotation, app.image_flip_h) {
        PreviewContent::Text(txt) => {
            app.native_preview.hide();
            let padded: Vec<String> = txt.lines().map(|line| format!("  {}", line)).collect();
            let para = Paragraph::new(padded.join("\n"))
                .style(Style::default().fg(C_TEXT_SOFT))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::Highlighted(lines) => {
            app.native_preview.hide();
            let mut padded_lines = Vec::new();
            for line in lines {
                let mut spans = line.spans;
                spans.insert(0, Span::raw("  "));
                padded_lines.push(Line::from(spans));
            }
            let para = Paragraph::new(Text::from(padded_lines))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::Code(lines) => {
            app.native_preview.hide();
            let mut padded_lines = Vec::new();
            for line in lines {
                let mut spans = line.spans;
                spans.insert(0, Span::raw("  "));
                padded_lines.push(Line::from(spans));
            }
            let para = Paragraph::new(Text::from(padded_lines))
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::ImageFallback(info) => {
            let mut top_margin = 0;
            if let Some(img) = &info.img {
                let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
                let mut img_rect = inner;
                if img_rect.height > 6 {
                    img_rect.y += 4;
                    img_rect.height -= 4;
                }
                if app.mode == AppMode::Normal {
                    app.native_preview.show(std::sync::Arc::clone(img), info.path.clone(), app.image_rotation, app.image_flip_h, img_rect, cols, rows);
                } else {
                    app.native_preview.hide();
                }
                top_margin = img_rect.y - inner.y;
            } else {
                app.native_preview.hide();
            }

            let mut text = vec![
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(&name, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD))
                ]),
            ];
            let mut meta_str = format!("{} Image  |  {}", ext, size_str);
            if let Some((w, h)) = info.dimensions {
                meta_str.push_str(&format!("  |  {} x {}", w, h));
            }
            text.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(meta_str, Style::default().fg(C_MUTED))
            ]));
            text.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("─".repeat((inner.width as usize).saturating_sub(4)), Style::default().fg(C_BORDER_LO))
            ]));

            for _ in 0..top_margin.saturating_sub(text.len() as u16) {
                text.push(Line::from(""));
            }

            let para = Paragraph::new(Text::from(text)).wrap(Wrap { trim: false }).scroll((scroll, 0));
            f.render_widget(para, inner);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// File icons
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a single-width glyph + color for a file extension. Deliberately a
/// small, curated set (not one emoji per extension) — multi-codepoint color
/// emoji render at inconsistent widths across terminal fonts, which is what
/// was causing icons to look misaligned/"off". These are plain BMP symbols
/// that stay one cell wide in any monospace font.
/// Color only — no icon glyph. After three rounds of "icon disappeared"
/// reports that traced back to the app running in legacy conhost.exe
/// (whose glyph coverage varies by font/mode in ways we can't fully control
/// from here), color-coding by category is the one visual differentiator
/// that is guaranteed to render identically everywhere: it only ever uses
/// plain ASCII text, just tinted.
struct IconSet {
    pub folder: &'static str,
    pub home: &'static str,
    pub desktop: &'static str,
    pub documents: &'static str,
    pub downloads: &'static str,
    pub pictures: &'static str,
    pub drive: &'static str,
    pub rs: &'static str,
    pub py: &'static str,
    pub js: &'static str,
    pub ts: &'static str,
    pub html: &'static str,
    pub css: &'static str,
    pub config: &'static str,
    pub shell: &'static str,
    pub c: &'static str,
    pub java: &'static str,
    pub go: &'static str,
    pub ruby: &'static str,
    pub php: &'static str,
    pub swift: &'static str,
    pub csharp: &'static str,
    pub lua: &'static str,
    pub sql: &'static str,
    pub text: &'static str,
    pub md: &'static str,
    pub pdf: &'static str,
    pub word: &'static str,
    pub excel: &'static str,
    pub ppt: &'static str,
    pub archive: &'static str,
    pub image: &'static str,
    pub video: &'static str,
    pub audio: &'static str,
    pub bin: &'static str,
    pub default: &'static str,
}

const EMOJI_ICONS: IconSet = IconSet {
    folder: "📂",
    home: "🏠",
    desktop: "🖥",
    documents: "📁",
    downloads: "📥",
    pictures: "🖼",
    drive: "💾",
    rs: "🦀",
    py: "🐍",
    js: "📜",
    ts: "📘",
    html: "🌐",
    css: "🎨",
    config: "⚙️",
    shell: "⚡",
    c: "🔧",
    java: "☕",
    go: "🐹",
    ruby: "💎",
    php: "🐘",
    swift: "🕊️",
    csharp: "🔷",
    lua: "🌙",
    sql: "🗄️",
    text: "📝",
    md: "📖",
    pdf: "📕",
    word: "📘",
    excel: "📊",
    ppt: "📊",
    archive: "📦",
    image: "🖼️",
    video: "🎬",
    audio: "🎵",
    bin: "🖥️",
    default: "📄",
};

const NERD_ICONS: IconSet = IconSet {
    folder: "",
    home: "",
    desktop: "",
    documents: "",
    downloads: "",
    pictures: "",
    drive: "",
    rs: "",
    py: "",
    js: "",
    ts: "",
    html: "",
    css: "",
    config: "",
    shell: "⚡",
    c: "",
    java: "",
    go: "",
    ruby: "",
    php: "",
    swift: "",
    csharp: "",
    lua: "",
    sql: "",
    text: "",
    md: "",
    pdf: "",
    word: "",
    excel: "",
    ppt: "",
    archive: "",
    image: "",
    video: "",
    audio: "",
    bin: "",
    default: "",
};

const ASCII_ICONS: IconSet = IconSet {
    folder: "d",
    home: "~",
    desktop: "D",
    documents: "d",
    downloads: "v",
    pictures: "p",
    drive: "c",
    rs: "R",
    py: "P",
    js: "J",
    ts: "T",
    html: "H",
    css: "C",
    config: "c",
    shell: ">",
    c: "c",
    java: "J",
    go: "G",
    ruby: "R",
    php: "p",
    swift: "S",
    csharp: "#",
    lua: "L",
    sql: "s",
    text: "t",
    md: "m",
    pdf: "F",
    word: "W",
    excel: "X",
    ppt: "P",
    archive: "a",
    image: "i",
    video: "v",
    audio: "a",
    bin: "x",
    default: "-",
};

fn get_icon_set() -> &'static IconSet {
    static SET: once_cell::sync::Lazy<IconSet> = once_cell::sync::Lazy::new(|| {
        let val = std::env::var("RUSTY_RANGER_ICONS").unwrap_or_default().to_lowercase();
        if val == "nerd" || std::env::var("NERD_FONT").is_ok() {
            NERD_ICONS
        } else if val == "ascii" {
            ASCII_ICONS
        } else {
            EMOJI_ICONS
        }
    });
    &SET
}

fn file_icon(ext: &str) -> (&'static str, Color) {
    const CODE:    Color = Color::Rgb(137, 180, 250); // blue
    const MARKUP:  Color = Color::Rgb(148, 226, 213); // teal
    const DOC:     Color = Color::Rgb(203, 166, 247); // lavender
    const SHEET:   Color = Color::Rgb(166, 227, 161); // green
    const IMAGE:   Color = Color::Rgb(245, 194, 231); // pink
    const MEDIA:   Color = Color::Rgb(250, 179, 135); // peach
    const ARCHIVE: Color = Color::Rgb(249, 226, 175); // yellow
    const BIN:     Color = Color::Rgb(243, 139, 168); // red
    const PLAIN:   Color = C_MUTED;

    let set = get_icon_set();
    match ext.to_lowercase().as_str() {
        // Specific languages with their iconic brand colors
        "rs" => (set.rs, Color::Rgb(224, 112, 16)), // orange
        "py" => (set.py, Color::Rgb(255, 224, 64)), // yellow
        "js"|"mjs"|"cjs" => (set.js, Color::Rgb(240, 219, 79)), // yellow
        "ts"|"tsx"|"jsx" => (set.ts, Color::Rgb(0, 122, 204)), // blue
        "html"|"htm" => (set.html, Color::Rgb(227, 76, 38)), // HTML red
        "css"|"scss"|"sass"|"less" => (set.css, Color::Rgb(86, 61, 124)), // CSS purple
        "go" => (set.go, Color::Rgb(0, 162, 232)), // blue
        "c"|"cpp"|"h"|"hpp"|"cc" => (set.c, Color::Rgb(63, 81, 181)), // blue
        "java"|"kt"|"kts" => (set.java, Color::Rgb(244, 67, 54)), // red
        "rb" => (set.ruby, Color::Rgb(204, 0, 0)), // red
        "php" => (set.php, Color::Rgb(119, 123, 179)), // blue
        "swift" => (set.swift, Color::Rgb(255, 102, 0)), // orange
        "cs" => (set.csharp, Color::Rgb(23, 150, 18)), // green
        "lua" => (set.lua, Color::Rgb(0, 0, 128)), // blue
        "sql" => (set.sql, Color::Rgb(0, 150, 136)), // teal

        // Config / Markup / Text
        "json"|"toml"|"yaml"|"yml"|"ini"|"cfg"|"conf" => (set.config, CODE),
        "sh"|"bash"|"zsh"|"ps1"|"bat"|"cmd" => (set.shell, Color::Rgb(255, 235, 59)), // yellow
        "txt"|"log" => (set.text, PLAIN),
        "md"|"markdown"|"rst"|"rtf" => (set.md, MARKUP),

        // Office & PDFs
        "pdf" => (set.pdf, BIN), // red
        "docx"|"doc"|"odt" => (set.word, CODE), // blue
        "xlsx"|"xls"|"ods"|"csv"|"tsv" => (set.excel, SHEET), // green
        "pptx"|"ppt"|"odp" => (set.ppt, MEDIA), // peach
        "ipynb" => (set.default, DOC), // notepad

        // Archives & Disks
        "zip"|"7z"|"rar"|"tar"|"gz"|"bz2"|"xz"|"zst"|"tgz" => (set.archive, ARCHIVE),
        "iso"|"img" => (set.default, ARCHIVE),

        // Media
        "jpg"|"jpeg"|"png"|"gif"|"bmp"|"webp"|"tiff"|"ico"|"svg" => (set.image, IMAGE),
        "mp4"|"mkv"|"avi"|"mov"|"webm"|"flv"|"wmv" => (set.video, MEDIA),
        "mp3"|"flac"|"wav"|"ogg"|"aac"|"m4a"|"opus" => (set.audio, MEDIA),

        // Binaries / Executables
        "exe"|"msi"|"apk"|"ipa" => (set.bin, BIN),
        "dll"|"so"|"dylib" => (set.bin, BIN),
        "ttf"|"otf"|"woff"|"woff2" => (set.default, PLAIN),

        _ => (set.default, PLAIN),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Status bar — also surfaces non-blocking notices
// ─────────────────────────────────────────────────────────────────────────────

fn draw_status_bar(f: &mut Frame, app: &AppState, area: Rect) {
    if let Some((msg, is_err)) = app.active_notice() {
        let style = if is_err {
            Style::default().fg(Color::White).bg(C_ERR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(C_OK).add_modifier(Modifier::BOLD)
        };
        let icon = if is_err { "[X]" } else { "[OK]" };
        f.render_widget(Paragraph::new(format!("  {} {}", icon, msg)).style(style), area);
        return;
    }

    let cur   = app.current();
    let count = cur.files.len();
    let pos   = if count > 0 { cur.selected + 1 } else { 0 };
    let marked = cur.marked.len();
    let sel_info = if marked > 0 { format!(" │ {} marked", marked) } else { String::new() };

    let bg = Color::Rgb(18, 19, 25);
    if app.mode != AppMode::Normal {
        let text = match app.mode {
            AppMode::Rename => format!(" {}/{} │ RENAME: type new name · Enter confirm · Esc cancel", pos, count.max(1)),
            AppMode::ConfirmDelete => format!(" {}/{} │ DELETE: Y confirm · Esc cancel", pos, count.max(1)),
            AppMode::ConfirmDeletePermanent => format!(" {}/{} │ PERMANENTLY DELETE: Y confirm · Esc cancel", pos, count.max(1)),
            AppMode::NewFolder => format!(" {}/{} │ NEW FOLDER: type name · Enter confirm · Esc cancel", pos, count.max(1)),
            AppMode::ContextMenu => format!(" {}/{} │ MENU: click an action · Esc close", pos, count.max(1)),
            AppMode::Properties => format!(" {}/{} │ PROPERTIES: Esc close", pos, count.max(1)),
            _ => String::new(),
        };
        let style = Style::default().fg(Color::Black).bg(C_WARN);
        f.render_widget(Paragraph::new(format!("  {}", text)).style(style), area);
        return;
    }

    let mut spans = vec![
        Span::styled(format!("  {}/{}{}  │  ", pos, count.max(1), sel_info), Style::default().fg(C_MUTED).bg(bg)),
    ];
    
    let key_style = Style::default().fg(C_TEXT).bg(bg).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(C_MUTED).bg(bg);
    
    spans.push(Span::styled("arrows", key_style));
    spans.push(Span::styled(" nav    ", label_style));
    
    spans.push(Span::styled("Shift+up/down", key_style));
    spans.push(Span::styled(" or ", label_style));
    spans.push(Span::styled("wheel", key_style));
    spans.push(Span::styled(" scroll preview    ", label_style));
    
    spans.push(Span::styled("F2", key_style));
    spans.push(Span::styled(" rename    ", label_style));
    
    spans.push(Span::styled("Ctrl+C/X/V", key_style));
    spans.push(Span::styled(" copy/cut/paste    ", label_style));
    
    spans.push(Span::styled("Del", key_style));
    spans.push(Span::styled(" delete    ", label_style));
    
    spans.push(Span::styled("RClick", key_style));
    spans.push(Span::styled(" menu    ", label_style));
    
    spans.push(Span::styled("q", key_style));
    spans.push(Span::styled(" quit", label_style));

    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)), area);
}
