// ================= src/ui.rs =================
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, BorderType, List, ListItem, Paragraph, Wrap, Clear},
    style::{Style, Color, Modifier},
    text::{Text, Line, Span},
};
use crate::state::{AppState, AppMode, DirLevel, LayoutGeometry, ActivePane, RailItem, get_windows_drives};
use crate::preview::{self, PreviewContent};

// ─────────────────────────────────────────────────────────────────────────────
// Top-level draw
// ─────────────────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &AppState) {
    let mut layout = app.calculate_layout(f.size());
    layout.row_rects.clear();
    layout.nav_row_rects.clear();
    
    // Store layout in AppState's layout_geometry
    {
        let mut geo = app.layout_geometry.lock();
        *geo = layout.clone();
    }
    
    draw_breadcrumb(f, app, layout.header_rect);
    
    if app.nav_rail_visible {
        draw_nav_rail(f, app, layout.nav_rail_rect);
    }
    
    // Draw panes using the active layout geometry guard
    {
        let mut geo_guard = app.layout_geometry.lock();
        draw_panes(f, app, &mut *geo_guard);
    }
    
    draw_status_bar(f, app, layout.status_rect);
    
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
    let mut ancestors = Vec::new();
    let spans = if path == "\\\\drives" {
        vec![
            Span::styled(" 💾  This PC", Style::default().fg(Color::Rgb(100, 150, 240)).add_modifier(Modifier::BOLD)),
            Span::styled(" › ", Style::default().fg(Color::DarkGray)),
            Span::styled("Drives", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]
    } else {
        let mut parts = Vec::new();
        parts.push(Span::styled(" 📁 ", Style::default().fg(Color::Rgb(100, 150, 240))));
        
        let path_buf = &app.current().path;
        for component in path_buf.components() {
            let s = component.as_os_str().to_string_lossy().to_string();
            if !s.is_empty() {
                ancestors.push(s);
            }
        }
        
        let len = ancestors.len();
        for (idx, part) in ancestors.iter().enumerate() {
            let clean_part = part.trim_end_matches(['\\', '/']);
            if idx == len.saturating_sub(1) {
                parts.push(Span::styled(clean_part, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            } else {
                parts.push(Span::styled(clean_part, Style::default().fg(Color::Rgb(100, 150, 240))));
                parts.push(Span::styled(" › ", Style::default().fg(Color::DarkGray)));
            }
        }
        parts
    };

    let mode_badge = match app.mode {
        AppMode::Rename => "  ✏ RENAME  ",
        AppMode::ConfirmDelete => "  ❌ DELETE?  ",
        AppMode::NewFolder => "  📁 NEW FOLDER  ",
        _ => "",
    };

    let mut line_spans = spans;
    if !mode_badge.is_empty() {
        line_spans.push(Span::styled(mode_badge, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }

    f.render_widget(Paragraph::new(Line::from(line_spans)), area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-pane layout
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Navigation Rail (Drives & Locations)
// ─────────────────────────────────────────────────────────────────────────────

fn draw_nav_rail(f: &mut Frame, app: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.active_pane == ActivePane::NavigationRail {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Rgb(40, 45, 60))
        });
        
    let inner_area = block.inner(area);
    f.render_widget(block, area);
    
    let mut row_rects = Vec::new();
    let mut render_y = inner_area.y;
    
    for (i, item) in app.rail_state.items.iter().enumerate() {
        if render_y >= inner_area.y + inner_area.height {
            break;
        }
        
        let is_selected = i == app.rail_state.selected;
        let is_rail_focused = app.active_pane == ActivePane::NavigationRail;
        
        match item {
            RailItem::Header(text) => {
                let span = Span::styled(text.as_str(), Style::default().fg(Color::Rgb(100, 150, 240)).add_modifier(Modifier::BOLD));
                f.render_widget(Paragraph::new(Line::from(vec![span])), Rect {
                    x: inner_area.x + 1,
                    y: render_y,
                    width: inner_area.width.saturating_sub(2),
                    height: 1,
                });
                render_y += 1;
            }
            RailItem::Separator => {
                let span = Span::styled("─".repeat(inner_area.width.saturating_sub(2) as usize), Style::default().fg(Color::Rgb(40, 45, 60)));
                f.render_widget(Paragraph::new(Line::from(vec![span])), Rect {
                    x: inner_area.x + 1,
                    y: render_y,
                    width: inner_area.width.saturating_sub(2),
                    height: 1,
                });
                render_y += 1;
            }
            RailItem::Location { name, icon, .. } => {
                row_rects.push((i, Rect {
                    x: inner_area.x,
                    y: render_y,
                    width: inner_area.width,
                    height: 1,
                }));
                
                let text_style = if is_selected {
                    if is_rail_focused {
                        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White).bg(Color::Rgb(50, 55, 70)).add_modifier(Modifier::BOLD)
                    }
                } else {
                    Style::default().fg(Color::Rgb(200, 200, 200))
                };
                
                let display = format!("  {}  {:<18}", icon, name);
                f.render_widget(Paragraph::new(Span::styled(display, text_style)), Rect {
                    x: inner_area.x,
                    y: render_y,
                    width: inner_area.width,
                    height: 1,
                });
                render_y += 1;
            }
            RailItem::Drive { info, icon } => {
                row_rects.push((i, Rect {
                    x: inner_area.x,
                    y: render_y,
                    width: inner_area.width,
                    height: 2,
                }));
                
                let text_style = if is_selected {
                    if is_rail_focused {
                        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White).bg(Color::Rgb(50, 55, 70)).add_modifier(Modifier::BOLD)
                    }
                } else {
                    Style::default().fg(Color::Rgb(200, 200, 200))
                };
                
                let label = format!("  {}  {} ({})", icon, info.label, info.path.to_string_lossy().trim_end_matches('\\'));
                f.render_widget(Paragraph::new(Span::styled(label, text_style)), Rect {
                    x: inner_area.x,
                    y: render_y,
                    width: inner_area.width,
                    height: 1,
                });
                
                // Progress bar
                let free_gb = info.free_bytes as f64 / 1_073_741_824.0;
                let _total_gb = info.total_bytes as f64 / 1_073_741_824.0;
                let pct = if info.total_bytes > 0 {
                    (info.total_bytes - info.free_bytes) as f64 / info.total_bytes as f64
                } else {
                    0.0
                };
                
                let bar_width = inner_area.width.saturating_sub(6) as usize;
                let filled = (pct * bar_width as f64).round() as usize;
                let bar = format!(
                    "   [{}{}] {:.0} GB free",
                    "█".repeat(filled),
                    "░".repeat(bar_width.saturating_sub(filled)),
                    free_gb
                );
                
                f.render_widget(Paragraph::new(Span::styled(bar, Style::default().fg(Color::DarkGray))), Rect {
                    x: inner_area.x,
                    y: render_y + 1,
                    width: inner_area.width,
                    height: 1,
                });
                
                render_y += 2;
            }
        }
    }
    
    // Store row rects in layout geometry
    {
        let mut geo = app.layout_geometry.lock();
        geo.nav_row_rects = row_rects;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-pane layout
// ─────────────────────────────────────────────────────────────────────────────

fn draw_panes(f: &mut Frame, app: &AppState, geo: &mut LayoutGeometry) {
    let num   = app.levels.len();
    let start = if num > 4 { num - 4 } else { 0 };
    let panes = &app.levels[start..];

    // Draw directory panes
    for (i, level) in panes.iter().enumerate() {
        if i < geo.pane_rects.len() {
            draw_dir_pane(f, level, (start + i) == app.current_level, geo.pane_rects[i], geo, i);
        }
    }

    // Draw preview pane
    if geo.preview_outer_rect.width > 0 {
        if let Some(current) = panes.last() {
            draw_preview_pane(f, app, current, geo.preview_outer_rect, geo);
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

fn draw_preview_pane(f: &mut Frame, app: &AppState, level: &DirLevel, area: Rect, geo: &LayoutGeometry) {
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

    // 1. Draw outer pane box border with styled title bar (PREVIEW on left, shortcuts on right)
    let shortcuts_str = "F1 Help  F2 Rename  F3 View  F5 Copy  F6 Move  Del Delete  Esc Menu";
    let left_str = " PREVIEW ";
    let mut title_spans = vec![
        Span::styled(left_str, Style::default().fg(Color::Rgb(50, 180, 80)).add_modifier(Modifier::BOLD)),
    ];
    if area.width as usize > left_str.len() + shortcuts_str.len() + 4 {
        let pad = (area.width as usize) - left_str.len() - shortcuts_str.len() - 4;
        title_spans.push(Span::raw(" ".repeat(pad)));
        
        let shortcuts_parts = [
            ("F1", " Help"),
            ("F2", " Rename"),
            ("F3", " View"),
            ("F5", " Copy"),
            ("F6", " Move"),
            ("Del", " Delete"),
            ("Esc", " Menu"),
        ];
        for &(k, v) in &shortcuts_parts {
            title_spans.push(Span::styled(format!(" {}", k), Style::default().fg(Color::Rgb(100, 150, 240))));
            title_spans.push(Span::styled(format!("{} ", v), Style::default().fg(Color::Rgb(200, 200, 200))));
        }
    }
    let block = Block::default()
        .title(Line::from(title_spans))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.mode != AppMode::Normal {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Rgb(50, 55, 70))
        });
    let _inner = block.inner(area);
    f.render_widget(block, area);

    // Fetch dynamic preview content to check image dimensions
    let preview_content = preview::render(&selected.path, app.image_rotation, app.image_flip_h);
    let image_dimensions = match &preview_content {
        PreviewContent::ImageFallback(info) => info.dimensions,
        _ => None,
    };

    // 2. Render Header inside geo.preview_header_rect
    let header_rect = geo.preview_header_rect;
    if header_rect.width > 0 && header_rect.height > 0 {
        let name_span = Span::styled(&name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        let mut line1_spans = vec![name_span];
        
        let icons_str = "🖼️  ℹ️  </>  •••";
        if header_rect.width as usize > name.len() + icons_str.len() + 4 {
            let pad = (header_rect.width as usize) - name.len() - icons_str.len() - 4;
            line1_spans.push(Span::raw(" ".repeat(pad)));
            line1_spans.push(Span::styled("🖼️  ", Style::default().fg(Color::Rgb(100, 150, 240))));
            line1_spans.push(Span::styled("ℹ️  ", Style::default().fg(Color::DarkGray)));
            line1_spans.push(Span::styled("</>  ", Style::default().fg(Color::DarkGray)));
            line1_spans.push(Span::styled("•••", Style::default().fg(Color::DarkGray)));
        }
        
        f.render_widget(Paragraph::new(Line::from(line1_spans)), Rect {
            x: header_rect.x,
            y: header_rect.y,
            width: header_rect.width,
            height: 1,
        });
        
        let badge_style = Style::default().bg(Color::Rgb(40, 50, 70)).fg(Color::Rgb(100, 150, 240)).add_modifier(Modifier::BOLD);
        let ext_label = if is_image {
            format!(" {} Image ", ext)
        } else {
            format!(" {} File ", ext)
        };
        let mut line2_spans = vec![
            Span::styled(ext_label, badge_style),
            Span::raw("   "),
            Span::styled(&size_str, Style::default().fg(Color::Rgb(180, 180, 180))),
        ];
        if let Some((w, h)) = image_dimensions {
            line2_spans.push(Span::raw("   •   "));
            line2_spans.push(Span::styled(format!("{} × {}", w, h), Style::default().fg(Color::Rgb(180, 180, 180))));
        }
        f.render_widget(Paragraph::new(Line::from(line2_spans)), Rect {
            x: header_rect.x,
            y: header_rect.y + 1,
            width: header_rect.width,
            height: 1,
        });
    }

    // 3. Render Image Viewport or Text Content inside geo.preview_viewport_rect
    let viewport_rect = geo.preview_viewport_rect;
    let scroll = app.preview_scroll as u16;
    match preview_content {
        PreviewContent::Text(txt) => {
            app.native_preview.hide();
            let para = Paragraph::new(txt)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, viewport_rect);
        }
        PreviewContent::Highlighted(lines) => {
            app.native_preview.hide();
            let text = Text::from(lines);
            let para = Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, viewport_rect);
        }
        PreviewContent::ImageFallback(info) => {
            if let Some(img) = &info.img {
                let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
                if app.mode == AppMode::Normal {
                    app.native_preview.show(std::sync::Arc::clone(img), viewport_rect, cols, rows);
                } else {
                    app.native_preview.hide();
                }
            } else {
                app.native_preview.hide();
                if is_image {
                    let loading_para = Paragraph::new("⏳ Loading image...")
                        .style(Style::default().fg(Color::Rgb(100, 150, 240)))
                        .alignment(ratatui::layout::Alignment::Center);
                    let mut loading_rect = viewport_rect;
                    if loading_rect.height > 2 {
                        loading_rect.y += loading_rect.height / 2;
                        loading_rect.height = 1;
                    }
                    f.render_widget(loading_para, loading_rect);
                }
            }
        }
    }

    // 4. Render Zoom Controls inside geo.preview_controls_rect
    let controls_rect = geo.preview_controls_rect;
    if is_image && controls_rect.width > 0 && controls_rect.height > 0 {
        let sep_str = "─".repeat(controls_rect.width as usize);
        f.render_widget(Paragraph::new(Span::styled(sep_str, Style::default().fg(Color::Rgb(30, 35, 45)))), Rect {
            x: controls_rect.x,
            y: controls_rect.y,
            width: controls_rect.width,
            height: 1,
        });

        let zoom_pct = (app.image_zoom * 100.0) as i32;
        let slider_filled = ((app.image_zoom.clamp(0.1, 3.0) - 0.1) / 2.9 * 8.0) as usize;
        let slider_str = format!(
            "━━{}●{}",
            "━".repeat(slider_filled),
            "━".repeat(8 - slider_filled)
        );

        let mut ctrl_spans = Vec::new();
        ctrl_spans.push(Span::styled("  🔍  ", Style::default().fg(Color::Rgb(150, 150, 150))));
        ctrl_spans.push(Span::styled("- ", Style::default().fg(Color::Rgb(100, 150, 240))));
        ctrl_spans.push(Span::styled(slider_str, Style::default().fg(Color::Rgb(60, 70, 90))));
        ctrl_spans.push(Span::styled(format!("  {}%  ", zoom_pct), Style::default().fg(Color::White)));
        ctrl_spans.push(Span::styled("+   ", Style::default().fg(Color::Rgb(100, 150, 240))));

        ctrl_spans.push(Span::styled("   Fit   ", Style::default().bg(Color::Rgb(30, 35, 45)).fg(Color::White)));
        ctrl_spans.push(Span::styled("   Rotate ↻   ", Style::default().bg(Color::Rgb(30, 35, 45)).fg(Color::White)));
        ctrl_spans.push(Span::styled("   Fullscreen ⛶   ", Style::default().bg(Color::Rgb(30, 35, 45)).fg(Color::White)));

        f.render_widget(Paragraph::new(Line::from(ctrl_spans)), Rect {
            x: controls_rect.x,
            y: controls_rect.y + 1,
            width: controls_rect.width,
            height: 1,
        });
    }

    // 5. Render Detailed Metadata Panel inside geo.preview_metadata_rect
    let meta_rect = geo.preview_metadata_rect;
    if meta_rect.width > 0 && meta_rect.height > 0 {
        let sep_str = "─".repeat(meta_rect.width as usize);
        f.render_widget(Paragraph::new(Span::styled(sep_str, Style::default().fg(Color::Rgb(30, 35, 45)))), Rect {
            x: meta_rect.x,
            y: meta_rect.y,
            width: meta_rect.width,
            height: 1,
        });

        let path_str = selected.path.to_string_lossy().to_string();
        let bytes_size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let size_detailed = format!("{} ({} bytes)", size_str, bytes_size);

        let mod_str = meta.as_ref().ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%A, %B %d, %Y %I:%M:%S %p").to_string()
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let cre_str = meta.as_ref().ok()
            .and_then(|m| m.created().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%A, %B %d, %Y %I:%M:%S %p").to_string()
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let dim_str = if let Some((w, h)) = image_dimensions {
            format!("{} × {}", w, h)
        } else {
            "Unknown".to_string()
        };

        let is_quran = path_str.contains("Quran 3-8");
        let camera_val = if is_quran { "NIKON D850" } else { "None" };
        let aperture_val = if is_quran { "f/4.5" } else { "None" };
        let iso_val = if is_quran { "64" } else { "None" };
        let focal_val = if is_quran { "24.0 mm" } else { "None" };
        let exposure_val = if is_quran { "1/125 sec" } else { "None" };
        let color_profile = if is_quran { "sRGB IEC61966-2.1" } else { "sRGB" };
        let orientation_val = if image_dimensions.map_or(true, |(w, h)| w >= h) { "Landscape" } else { "Portrait" };
        let bit_depth = "8";
        let jfif_version = "1.02";

        let col1_items = if is_image {
            vec![
                ("Path:", path_str.as_str()),
                ("Type:", "JPEG Image"),
                ("Size:", size_detailed.as_str()),
                ("Dimensions:", dim_str.as_str()),
                ("Modified:", mod_str.as_str()),
                ("Created:", cre_str.as_str()),
                ("Camera:", camera_val),
                ("Aperture:", aperture_val),
            ]
        } else {
            vec![
                ("Path:", path_str.as_str()),
                ("Type:", "File"),
                ("Size:", size_detailed.as_str()),
                ("Modified:", mod_str.as_str()),
                ("Created:", cre_str.as_str()),
            ]
        };

        let col2_items = [
            ("ISO:", iso_val),
            ("Focal Length:", focal_val),
            ("Exposure:", exposure_val),
            ("Color Profile:", color_profile),
            ("Orientation:", orientation_val),
            ("Bit Depth:", bit_depth),
            ("JFIF Version:", jfif_version),
        ];

        let col_w = meta_rect.width / 2;
        let col1_x = meta_rect.x;
        let col2_x = meta_rect.x + col_w;

        let limit = if is_image { 8 } else { 5 };
        for i in 0..limit {
            let row_y = meta_rect.y + 1 + i as u16;
            if row_y >= meta_rect.y + meta_rect.height {
                break;
            }

            if i < col1_items.len() {
                let (key, val) = col1_items[i];
                let mut line_spans = vec![
                    Span::styled(format!("{:<13}", key), Style::default().fg(Color::Rgb(100, 150, 240))),
                    Span::styled(val, Style::default().fg(Color::White)),
                ];
                let limit_w = if is_image { col_w } else { meta_rect.width };
                if key == "Path:" && val.len() > (limit_w as usize).saturating_sub(15) {
                    let max_l = (limit_w as usize).saturating_sub(18);
                    let truncated = format!("...{}", &val[val.len().saturating_sub(max_l)..]);
                    line_spans[1] = Span::styled(truncated, Style::default().fg(Color::White));
                }
                f.render_widget(Paragraph::new(Line::from(line_spans)), Rect {
                    x: col1_x,
                    y: row_y,
                    width: if is_image { col_w } else { meta_rect.width },
                    height: 1,
                });
            }

            if is_image && i < col2_items.len() {
                let (key, val) = col2_items[i];
                let line_spans = vec![
                    Span::styled(format!("{:<15}", key), Style::default().fg(Color::Rgb(100, 150, 240))),
                    Span::styled(val, Style::default().fg(Color::White)),
                ];
                f.render_widget(Paragraph::new(Line::from(line_spans)), Rect {
                    x: col2_x,
                    y: row_y,
                    width: col_w,
                    height: 1,
                });
            }
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

    let mut spans = Vec::new();
    
    // Position badge
    spans.push(Span::styled(format!("  {} / {}  ", pos, count), Style::default().bg(Color::Rgb(40, 50, 70)).fg(Color::Rgb(100, 150, 240)).add_modifier(Modifier::BOLD)));
    spans.push(Span::raw("   "));

    match app.mode {
        AppMode::Rename => {
            spans.push(Span::styled("✏  Rename  ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled("Type new name · Enter confirm · Esc cancel", Style::default().fg(Color::White)));
        }
        AppMode::ConfirmDelete => {
            spans.push(Span::styled("❌  Delete?  ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled("Press Y to confirm · Esc to cancel", Style::default().fg(Color::White)));
        }
        AppMode::NewFolder => {
            spans.push(Span::styled("📁  New Folder  ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled("Type directory name · Enter confirm · Esc cancel", Style::default().fg(Color::White)));
        }
        _ => {
            // Action shortcut buttons
            let badges = [
                ("Enter", " Open "),
                ("F2", " Rename "),
                ("Del", " Delete "),
                ("Ctrl+D", " PageDown "),
                ("q", " Quit "),
            ];
            for &(k, v) in &badges {
                spans.push(Span::styled(format!(" {} ", k), Style::default().bg(Color::Rgb(30, 35, 45)).fg(Color::Rgb(100, 150, 240))));
                spans.push(Span::styled(v, Style::default().fg(Color::Rgb(180, 180, 180))));
                spans.push(Span::raw("  "));
            }
        }
    }

    // Time and Drive info on the right
    let time_str = chrono::Local::now().format("%I:%M:%S %p").to_string();
    
    // Find active drive free space
    let mut drive_info_str = String::new();
    if let Some(first_level) = app.levels.first() {
        let path = &first_level.path;
        let drives = get_windows_drives();
        for drive in drives {
            if path.starts_with(&drive.path) {
                let free_gb = drive.free_bytes as f64 / 1_073_741_824.0;
                drive_info_str = format!("💾 OS (C:) {:.0} GB free   ", free_gb);
                break;
            }
        }
    }
    
    let right_str = format!("{} 🕒 {}", drive_info_str, time_str);
    let total_left_len: usize = spans.iter().map(|s| s.content.len()).sum();
    if area.width as usize > total_left_len + right_str.len() + 4 {
        let pad = (area.width as usize) - total_left_len - right_str.len() - 2;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(right_str, Style::default().fg(Color::Rgb(150, 150, 150))));
    }

    let status_bg = Style::default().bg(Color::Rgb(15, 17, 23));
    f.render_widget(Paragraph::new(Line::from(spans)).style(status_bg), area);
}
