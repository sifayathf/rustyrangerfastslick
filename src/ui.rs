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
const C_ACCENT:   Color = Color::Rgb(97, 214, 214);   // cyan focus accent
const C_ACCENT2:  Color = Color::Rgb(137, 180, 250);  // soft blue

const C_BORDER:   Color = Color::Rgb(58, 62, 78);
const C_BORDER_LO:Color = Color::Rgb(40, 43, 54);
const C_TEXT:     Color = Color::Rgb(214, 218, 230);
const C_MUTED:    Color = Color::Rgb(120, 126, 145);
const C_WARN:     Color = Color::Rgb(240, 198, 116);
const C_OK:       Color = Color::Rgb(137, 220, 165);
const C_ERR:      Color = Color::Rgb(240, 120, 120);
const C_SEL_BG:   Color = Color::Rgb(45, 90, 92);
const C_SEL_BG_INACTIVE: Color = Color::Rgb(52, 55, 68);
const C_MARK:     Color = Color::Rgb(210, 160, 90);

// ─────────────────────────────────────────────────────────────────────────────
// Top-level draw
// ─────────────────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // content + sidebar
            Constraint::Length(1), // status bar
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

fn draw_sidebar(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    let block = Block::default()
        .title(" DRIVES & LOCATIONS ")
        .title_style(Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_BORDER_LO));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cur_path = &app.current().path;
    let mut y = inner.y;
    let max_y = inner.y + inner.height;

    push_line(f, geo, inner, &mut y, max_y,
        Line::from(Span::styled(" QUICK ACCESS", Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD))), None);
    for (label, path) in app.quick_access.iter() {
        let is_active = *path == *cur_path;
        let text_style = if is_active {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_TEXT)
        };
        push_line(f, geo, inner, &mut y, max_y,
            Line::from(vec![
                Span::styled(format!("  {}", label), text_style),
            ]),
            Some(path.clone()));
    }

    y += 1;
    push_line(f, geo, inner, &mut y, max_y,
        Line::from(Span::styled(" DRIVES", Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD))), None);

    for d in app.drives.iter() {
        if y + 1 >= max_y { break; }
        let is_active = d.path == *cur_path;
        let name_style = if is_active {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_TEXT)
        };
        let icon = match d.kind.as_str() {
            "Removable" => "🔌",
            "CD-ROM"    => "💿",
            "Network"   => "🌐",
            _           => "💾",
        };
        let letter = d.path.to_string_lossy().trim_end_matches('\\').to_string();
        
        let label_text = truncate(&d.label, inner.width.saturating_sub(10) as usize);
        push_line(f, geo, inner, &mut y, max_y,
            Line::from(vec![
                Span::styled(format!("  {} {} ", icon, letter), name_style),
                Span::styled(label_text, Style::default().fg(C_MUTED)),
            ]),
            Some(d.path.clone()));

        if d.total > 0 {
            let used = d.total.saturating_sub(d.free);
            let frac = (used as f64 / d.total as f64).clamp(0.0, 1.0);
            let total_gb = d.total as f64 / 1_073_741_824.0;
            push_line(f, geo, inner, &mut y, max_y,
                Line::from(vec![
                    Span::styled(format!("     {:.0}% of {:.0}GB free", (1.0 - frac) * 100.0, total_gb), Style::default().fg(C_MUTED)),
                ]),
                None);
        }
    }
}

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
    if max <= 1 { return "…".to_string(); }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
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
        let label = if matches!(action, ContextAction::Delete) {
            Span::styled(format!(" {}", action.label()), Style::default().fg(C_ERR))
        } else {
            Span::styled(format!(" {}", action.label()), Style::default().fg(C_TEXT))
        };
        f.render_widget(Paragraph::new(Line::from(label)), rect);
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
            Paragraph::new(Span::styled(" ◉  This PC — Drives", Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))),
            area,
        );
        return;
    }

    // Build clickable segments: C:\ > Users > sifay > Pictures
    let mut spans = vec![Span::styled(" ▸  ", Style::default().fg(C_MUTED))];
    let mut segments: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut acc = std::path::PathBuf::new();
    for comp in path.components() {
        acc.push(comp.as_os_str());
        let label = comp.as_os_str().to_string_lossy().to_string();
        segments.push((label, acc.clone()));
    }

    let mut x = area.x + 5;
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
            spans.push(Span::styled("  ›  ", Style::default().fg(C_MUTED)));
            x += 5;
        }
    }

    let mode_badge = match app.mode {
        AppMode::Rename => "  ✎ RENAME",
        AppMode::ConfirmDelete | AppMode::ConfirmDeletePermanent => "  ✖ DELETE?",
        AppMode::NewFolder => "  ▸ NEW FOLDER",
        AppMode::ContextMenu => "  ☰ MENU",
        AppMode::Properties => "  ℹ PROPERTIES",
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

            let icon = if p.is_dir {
                "📂"
            } else {
                let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                file_icon(ext)
            };
            let is_marked = level.marked.contains(&p.path);
            let is_selected = file_i == level.selected;

            if is_selected {
                let bg = if is_current { C_SEL_BG } else { C_SEL_BG_INACTIVE };
                let fg_color = if is_current { C_ACCENT } else { C_TEXT };
                let inner_w = (area.width as usize).saturating_sub(2);
                let disp_w = 2 + shown_name.chars().count(); // icon + space + name
                let pad_w = inner_w.saturating_sub(1).saturating_sub(disp_w);
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", icon), Style::default().bg(bg)),
                    Span::styled(format!("{}{}", shown_name, " ".repeat(pad_w)), Style::default().fg(fg_color).bg(bg).add_modifier(Modifier::BOLD)),
                ]))
            } else if is_marked {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", icon), Style::default()),
                    Span::styled(shown_name, Style::default().fg(C_MARK).add_modifier(Modifier::BOLD)),
                ]))
            } else {
                let style = if is_current { Style::default().fg(C_TEXT) } else { Style::default().fg(C_MUTED) };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", icon), Style::default()),
                    Span::styled(shown_name, style),
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
                let name = truncate(&p.path.file_name().unwrap_or_else(|| p.path.as_os_str()).to_string_lossy(), name_budget.max(4));
                let icon = if p.is_dir {
                    "📂"
                } else {
                    let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    file_icon(ext)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default()),
                    Span::styled(name, Style::default().fg(C_MUTED)),
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
            let para = Paragraph::new(txt).wrap(Wrap { trim: false }).scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::Highlighted(lines) => {
            app.native_preview.hide();
            let text = Text::from(lines);
            let para = Paragraph::new(text).wrap(Wrap { trim: false }).scroll((scroll, 0));
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
                Line::from(vec![Span::styled(&name, Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD))]),
            ];
            let mut meta_str = format!("{} Image  •  {}", ext, size_str);
            if let Some((w, h)) = info.dimensions {
                meta_str.push_str(&format!("  •  {} × {}", w, h));
            }
            text.push(Line::from(vec![Span::styled(meta_str, Style::default().fg(C_MUTED))]));
            text.push(Line::from(vec![Span::styled("─".repeat(inner.width as usize), Style::default().fg(C_BORDER_LO))]));

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
// Status bar — also surfaces non-blocking notices
// ─────────────────────────────────────────────────────────────────────────────

fn draw_status_bar(f: &mut Frame, app: &AppState, area: Rect) {
    if let Some((msg, is_err)) = app.active_notice() {
        let style = if is_err {
            Style::default().fg(Color::White).bg(C_ERR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(C_OK).add_modifier(Modifier::BOLD)
        };
        let icon = if is_err { "✖" } else { "✓" };
        f.render_widget(Paragraph::new(format!(" {} {}", icon, msg)).style(style), area);
        return;
    }

    let cur   = app.current();
    let count = cur.files.len();
    let pos   = if count > 0 { cur.selected + 1 } else { 0 };
    let marked = cur.marked.len();
    let sel_info = if marked > 0 { format!(" │ {} marked", marked) } else { String::new() };

    let text = match app.mode {
        AppMode::Rename => format!(" {}/{} │ ✏ RENAME: type new name · Enter confirm · Esc cancel", pos, count.max(1)),
        AppMode::ConfirmDelete => format!(" {}/{} │ ✖ DELETE: Y confirm · Esc cancel", pos, count.max(1)),
        AppMode::ConfirmDeletePermanent => format!(" {}/{} │ ✖ PERMANENTLY DELETE: Y confirm · Esc cancel", pos, count.max(1)),
        AppMode::NewFolder => format!(" {}/{} │ ▸ NEW FOLDER: type name · Enter confirm · Esc cancel", pos, count.max(1)),
        AppMode::ContextMenu => format!(" {}/{} │ ☰ MENU: click an action · Esc close", pos, count.max(1)),
        AppMode::Properties => format!(" {}/{} │ ℹ PROPERTIES: Esc close", pos, count.max(1)),
        _ => format!(
            " {}/{}{} │ ←↓↑→ nav  F2 rename  Ctrl+C/X/V copy/cut/paste  Del delete  Ctrl+N folder  RClick menu  q quit",
            pos, count.max(1), sel_info
        ),
    };

    let style = if app.mode != AppMode::Normal {
        Style::default().fg(Color::Black).bg(C_WARN)
    } else {
        Style::default().fg(C_MUTED).bg(Color::Rgb(18, 19, 25))
    };

    f.render_widget(Paragraph::new(text).style(style), area);
}
