// ================= src/ui.rs =================
#![allow(non_snake_case)]
use crate::preview::{self, PreviewContent};
use crate::state::{
    AppMode, AppState, ContextAction, DirLevel, LayoutGeometry, LayoutMode, PreviewMode, SortMode,
    ThemeMode,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// Full-cell Powerline caps used by the original oval implementation. Standard
// circle halves (◗/◖) are much shorter than a terminal row and look like dots.
const PILL_LEFT_CAP: &str = "";
const PILL_RIGHT_CAP: &str = "";
const PAGE_PREV_LABEL: &str = "[< Prev]";
const PAGE_NEXT_LABEL: &str = "[Next >]";

fn page_control_rects(inner: Rect) -> (Rect, Rect) {
    let prev_x = inner.x.saturating_add(2);
    let next_x = prev_x.saturating_add(PAGE_PREV_LABEL.len() as u16 + 2);
    (
        Rect {
            x: prev_x,
            y: inner.y.saturating_add(2),
            width: PAGE_PREV_LABEL.len() as u16,
            height: 1,
        },
        Rect {
            x: next_x,
            y: inner.y.saturating_add(2),
            width: PAGE_NEXT_LABEL.len() as u16,
            height: 1,
        },
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Top-level draw
// ─────────────────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &AppState) {
    let t = app.theme();
    let C_BORDER_LO = t.border_lo;
    let root_style = if app.font_weight >= 600 {
        Style::default()
            .fg(t.text)
            .bg(t.bg_root)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.text).bg(t.bg_root)
    };
    f.render_widget(Block::default().style(root_style), f.size());

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
    geo.pane_level_indices.clear();
    geo.preview_rect = None;
    geo.row_rects.clear();
    geo.tile_columns = 1;
    geo.tile_visible_items = 0;
    geo.divider_rects.clear();
    geo.sidebar_item_rects.clear();
    geo.toggle_rects.clear();
    geo.preview_dir_item_rects.clear();
    geo.edit_save_btn_rect = None;
    geo.slide_prev_rect = None;
    geo.slide_next_rect = None;
    geo.breadcrumb_segment_rects.clear();
    geo.search_rect = None;
    geo.pane_sort_rect = None;
    geo.context_menu_item_rects.clear();
    geo.context_menu_rect = None;

    // Responsive: hide the sidebar on very narrow terminals so panes stay usable.
    let show_sidebar = root[0].width >= 90;
    let sidebar_w = if show_sidebar {
        let maximum = root[0].width.saturating_sub(48).clamp(18, 36);
        app.sidebar_width.clamp(18, maximum)
    } else {
        0
    };

    let body = if show_sidebar {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(sidebar_w),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(root[0])
    } else {
        Layout::default()
            .constraints([Constraint::Min(0)])
            .split(root[0])
    };

    if show_sidebar {
        geo.sidebar_rect = body[0];
        geo.sidebar_divider_rect = body[1];
        draw_sidebar(f, app, body[0], &mut geo);
        draw_vertical_divider(f, body[1], C_BORDER_LO);
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

    if matches!(
        app.mode,
        AppMode::Rename | AppMode::NewFolder | AppMode::NewFile
    ) {
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

fn draw_vertical_divider(f: &mut Frame, area: Rect, border_color: Color) {
    for y in area.y..area.y + area.height {
        f.render_widget(
            Paragraph::new(Span::styled("│", Style::default().fg(border_color))),
            Rect {
                x: area.x,
                y,
                width: 1,
                height: 1,
            },
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Left sidebar: Quick Access + Drives + Toggles
// ─────────────────────────────────────────────────────────────────────────────

fn draw_sidebar(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    let t = app.theme();
    let C_ACCENT = t.accent;
    let C_ACCENT2 = t.accent2;
    let C_BG_PANEL = t.bg_panel;
    let C_BORDER_LO = t.border_lo;
    let C_TEXT = t.text;
    let C_MUTED = t.muted;
    let C_WARN = t.warn;
    let C_OK = t.ok;
    let C_ERR = t.err;

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

    push_line(
        f,
        geo,
        inner,
        &mut y,
        max_y,
        Line::from(Span::styled(
            "  QUICK ACCESS",
            Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
        )),
        None,
    );

    let set = get_icon_set();
    for (label, path) in app.quick_access.iter() {
        let is_active = *path == *cur_path;
        let text_style = if is_active {
            Style::default()
                .fg(Color::Black)
                .bg(C_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_TEXT)
        };
        // Render the icon and label into independent rectangles. This keeps
        // the label at a fixed column even when a terminal displays an emoji
        // using a different fallback-font width.
        let icon = if *label == "Home" {
            set.home
        } else if *label == "Desktop" {
            set.desktop
        } else if *label == "Documents" {
            set.documents
        } else if *label == "Downloads" {
            set.downloads
        } else if *label == "Pictures" {
            set.pictures
        } else {
            " "
        };
        if y >= max_y {
            break;
        }
        let row_rect = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: 1,
        };
        if is_active {
            if app.rounded_selection && row_rect.width >= 2 {
                let body = Rect {
                    x: row_rect.x.saturating_add(1),
                    y,
                    width: row_rect.width.saturating_sub(2),
                    height: 1,
                };
                f.render_widget(Paragraph::new("").style(text_style), body);
                f.render_widget(
                    Paragraph::new(Span::styled(
                        PILL_LEFT_CAP,
                        Style::default().fg(C_ACCENT).bg(t.bg_panel),
                    )),
                    Rect {
                        x: row_rect.x,
                        y,
                        width: 1,
                        height: 1,
                    },
                );
                f.render_widget(
                    Paragraph::new(Span::styled(
                        PILL_RIGHT_CAP,
                        Style::default().fg(C_ACCENT).bg(t.bg_panel),
                    )),
                    Rect {
                        x: row_rect.x.saturating_add(row_rect.width.saturating_sub(1)),
                        y,
                        width: 1,
                        height: 1,
                    },
                );
            } else {
                f.render_widget(Paragraph::new("").style(text_style), row_rect);
            }
        }
        let icon_rect = Rect {
            x: inner.x.saturating_add(2),
            y,
            width: 2.min(inner.width.saturating_sub(2)),
            height: 1,
        };
        let label_rect = Rect {
            x: inner.x.saturating_add(5),
            y,
            width: inner.width.saturating_sub(5),
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                icon,
                if is_active {
                    text_style
                } else {
                    Style::default().fg(C_ACCENT2)
                },
            )),
            icon_rect,
        );
        f.render_widget(Paragraph::new(Span::styled(*label, text_style)), label_rect);
        geo.sidebar_item_rects.push((row_rect, path.clone()));
        y += 1;
    }

    // Spacing between sections
    y += 1;
    push_line(
        f,
        geo,
        inner,
        &mut y,
        max_y,
        Line::from(Span::styled(
            "  DRIVES",
            Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
        )),
        None,
    );

    for d in app.drives.iter() {
        if y + 2 >= max_y {
            break;
        }
        let is_active = d.path == *cur_path;
        let dot_color = match d.kind.as_str() {
            "Removable" => C_WARN,
            "Network" => C_OK,
            "CD-ROM" => C_MUTED,
            _ => C_ACCENT2,
        };
        let letter_color = if is_active { C_ACCENT } else { dot_color };
        let letter = d.path.to_string_lossy().trim_end_matches('\\').to_string();

        let icon_span = Span::styled(set.drive, Style::default().fg(letter_color));
        let text_span = Span::styled(
            format!("  {}  ", letter),
            Style::default()
                .fg(letter_color)
                .add_modifier(Modifier::BOLD),
        );

        push_line(
            f,
            geo,
            inner,
            &mut y,
            max_y,
            Line::from(vec![
                Span::styled("  ", Style::default()),
                icon_span,
                text_span,
                Span::styled(
                    truncate(
                        &if d.fs.is_empty() {
                            d.label.clone()
                        } else {
                            format!("{} · {}", d.label, d.fs)
                        },
                        inner.width.saturating_sub(13) as usize,
                    ),
                    Style::default().fg(C_MUTED),
                ),
            ]),
            Some(d.path.clone()),
        );

        if d.total > 0 {
            let used = d.total.saturating_sub(d.free);
            let frac = (used as f64 / d.total as f64).clamp(0.0, 1.0);
            let bar_w = inner.width.saturating_sub(4) as usize;
            let filled = ((bar_w as f64) * frac).round() as usize;
            let bar_color = if frac > 0.9 {
                C_ERR
            } else if frac > 0.75 {
                C_WARN
            } else {
                C_ACCENT2
            };
            let mut bar = String::new();
            bar.push_str(&"█".repeat(filled.min(bar_w)));
            bar.push_str(&"░".repeat(bar_w.saturating_sub(filled)));
            let free_gb = d.free as f64 / 1_073_741_824.0;
            let total_gb = d.total as f64 / 1_073_741_824.0;
            push_line(
                f,
                geo,
                inner,
                &mut y,
                max_y,
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(bar, Style::default().fg(bar_color)),
                ]),
                None,
            );
            push_line(
                f,
                geo,
                inner,
                &mut y,
                max_y,
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(
                        format!("{:.0} GB free of {:.0} GB", free_gb, total_gb),
                        Style::default().fg(C_MUTED),
                    ),
                ]),
                None,
            );
        }
    }

    // ── TOGGLES & SETTINGS (in the empty space below Drives) ─────────────
    if y + 8 <= max_y {
        y += 1;
        push_line(
            f,
            geo,
            inner,
            &mut y,
            max_y,
            Line::from(Span::styled(
                "  TOGGLES & SETTINGS",
                Style::default().fg(C_MUTED).add_modifier(Modifier::BOLD),
            )),
            None,
        );

        let draw_toggle = |f: &mut Frame,
                           geo: &mut LayoutGeometry,
                           y: &mut u16,
                           icon: &'static str,
                           label: &'static str,
                           opt1: &'static str,
                           opt2: &'static str,
                           is_opt2: bool,
                           action: crate::state::ToggleAction| {
            if *y >= max_y {
                return;
            }
            let rect = Rect {
                x: inner.x,
                y: *y,
                width: inner.width,
                height: 1,
            };

            // Keep the label/control pair inside the narrow sidebar. Leading
            // padding here previously forced the second pill into the next row.
            let label_span = Span::styled(
                format!("{} {:<6}", icon, label),
                Style::default().fg(C_TEXT),
            );
            let badge_style = |active: bool| {
                if active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(C_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_MUTED).bg(t.sel_bg_inactive)
                }
            };
            let cap_style = |active: bool| {
                let color = if active { C_ACCENT } else { t.sel_bg_inactive };
                if app.rounded_selection {
                    Style::default().fg(color).bg(t.bg_panel)
                } else {
                    Style::default().bg(color)
                }
            };
            let left_cap = if app.rounded_selection {
                PILL_LEFT_CAP
            } else {
                " "
            };
            let right_cap = if app.rounded_selection {
                PILL_RIGHT_CAP
            } else {
                " "
            };

            let line = Line::from(vec![
                label_span,
                Span::styled(left_cap, cap_style(!is_opt2)),
                Span::styled(opt1, badge_style(!is_opt2)),
                Span::styled(right_cap, cap_style(!is_opt2)),
                Span::raw(" "),
                Span::styled(left_cap, cap_style(is_opt2)),
                Span::styled(opt2, badge_style(is_opt2)),
                Span::styled(right_cap, cap_style(is_opt2)),
            ]);

            f.render_widget(Paragraph::new(line), rect);
            geo.toggle_rects.push((rect, action));
            *y += 1;
        };

        let draw_value = |f: &mut Frame,
                          geo: &mut LayoutGeometry,
                          y: &mut u16,
                          icon: &'static str,
                          label: &'static str,
                          value: String,
                          action: crate::state::ToggleAction| {
            if *y >= max_y {
                return;
            }
            let rect = Rect {
                x: inner.x,
                y: *y,
                width: inner.width,
                height: 1,
            };
            let label_span = Span::styled(
                format!("{} {:<6}", icon, label),
                Style::default().fg(C_TEXT),
            );
            let value_style = Style::default()
                .fg(Color::Black)
                .bg(C_ACCENT)
                .add_modifier(Modifier::BOLD);
            let cap_style = if app.rounded_selection {
                Style::default().fg(C_ACCENT).bg(t.bg_panel)
            } else {
                Style::default().bg(C_ACCENT)
            };
            let left_cap = if app.rounded_selection {
                PILL_LEFT_CAP
            } else {
                " "
            };
            let right_cap = if app.rounded_selection {
                PILL_RIGHT_CAP
            } else {
                " "
            };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    label_span,
                    Span::styled(left_cap, cap_style),
                    Span::styled(value, value_style),
                    Span::styled(right_cap, cap_style),
                ])),
                rect,
            );
            geo.toggle_rects.push((rect, action));
            *y += 1;
        };

        draw_toggle(
            f,
            geo,
            &mut y,
            "🎨",
            "Theme",
            "Dark",
            "Light",
            app.theme_mode == crate::state::ThemeMode::Light,
            crate::state::ToggleAction::Theme,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "▣",
            "View",
            "Columns",
            "Tiles",
            app.layout_mode == LayoutMode::Explorer,
            crate::state::ToggleAction::LayoutMode,
        );
        if y.saturating_add(1) < max_y {
            let label_rect = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Span::styled("⚡ Mode", Style::default().fg(C_TEXT))),
                label_rect,
            );
            y += 1;

            let options_rect = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            let options = [
                (
                    "Normal",
                    PreviewMode::Normal,
                    crate::state::ToggleAction::PreviewNormal,
                ),
                (
                    "Full",
                    PreviewMode::Full,
                    crate::state::ToggleAction::PreviewFull,
                ),
                (
                    "Blitz",
                    PreviewMode::Blitz,
                    crate::state::ToggleAction::PreviewBlitz,
                ),
            ];
            let mut spans = vec![Span::raw(" ")];
            let mut option_x = options_rect.x.saturating_add(1);
            for (index, (label, mode, action)) in options.into_iter().enumerate() {
                if index > 0 {
                    spans.push(Span::raw(" "));
                    option_x = option_x.saturating_add(1);
                }
                let active = app.preview_mode == mode;
                let color = if active { C_ACCENT } else { t.sel_bg_inactive };
                let badge_style = if active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(color)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_MUTED).bg(color)
                };
                let cap_style = if app.rounded_selection {
                    Style::default().fg(color).bg(t.bg_panel)
                } else {
                    Style::default().bg(color)
                };
                let left_cap = if app.rounded_selection {
                    PILL_LEFT_CAP
                } else {
                    " "
                };
                let right_cap = if app.rounded_selection {
                    PILL_RIGHT_CAP
                } else {
                    " "
                };
                spans.push(Span::styled(left_cap, cap_style));
                spans.push(Span::styled(label, badge_style));
                spans.push(Span::styled(right_cap, cap_style));
                let width = label.width() as u16 + 2;
                geo.toggle_rects.push((
                    Rect {
                        x: option_x,
                        y,
                        width,
                        height: 1,
                    },
                    action,
                ));
                option_x = option_x.saturating_add(width);
            }
            f.render_widget(Paragraph::new(Line::from(spans)), options_rect);
            y += 1;
        }
        draw_toggle(
            f,
            geo,
            &mut y,
            "📊",
            "Office",
            "Text",
            "Full",
            app.effective_office_mode() == crate::state::OfficeRenderMode::Full,
            crate::state::ToggleAction::OfficeMode,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "📄",
            "PDF",
            "Text",
            "Visual",
            app.effective_pdf_mode() == crate::state::PdfRenderMode::Visual,
            crate::state::ToggleAction::PdfMode,
        );
        if y < max_y {
            let rect = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            };
            let prefix = Span::styled("↕ ", Style::default().fg(C_TEXT));
            let mut click_x = rect.x + prefix.width() as u16;
            let outer_color = t.sel_bg_inactive;
            let outer_style = if app.rounded_selection {
                Style::default().fg(outer_color).bg(t.bg_panel)
            } else {
                Style::default().bg(outer_color)
            };
            let left_cap = if app.rounded_selection {
                PILL_LEFT_CAP
            } else {
                " "
            };
            let right_cap = if app.rounded_selection {
                PILL_RIGHT_CAP
            } else {
                " "
            };
            let mut spans = vec![prefix, Span::styled(left_cap, outer_style)];
            click_x = click_x.saturating_add(1);
            for (label, mode, action) in [
                ("Name", SortMode::Name, crate::state::ToggleAction::SortName),
                (
                    "Date",
                    SortMode::Modified,
                    crate::state::ToggleAction::SortModified,
                ),
                ("Size", SortMode::Size, crate::state::ToggleAction::SortSize),
            ] {
                let style = if app.sort_mode == mode {
                    Style::default()
                        .fg(Color::Black)
                        .bg(C_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_MUTED).bg(t.sel_bg_inactive)
                };
                // Explicit interior spacing makes all three sort targets
                // visually distinct and independently clickable.
                let badge = format!("{} ", label);
                let badge_width = Span::raw(&badge).width() as u16;
                geo.toggle_rects.push((
                    Rect {
                        x: click_x,
                        y,
                        width: badge_width,
                        height: 1,
                    },
                    action,
                ));
                spans.push(Span::styled(badge, style));
                click_x = click_x.saturating_add(badge_width);
            }
            spans.push(Span::styled(right_cap, outer_style));
            f.render_widget(Paragraph::new(Line::from(spans)), rect);
            y += 1;
        }
        draw_toggle(
            f,
            geo,
            &mut y,
            "↕",
            "Order",
            "Asc",
            "Desc",
            app.sort_descending,
            crate::state::ToggleAction::SortOrder,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "🕒",
            "Details",
            "Off",
            "On",
            app.show_file_details,
            crate::state::ToggleAction::Details,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "●",
            "Shape",
            "Flat",
            "Pill",
            app.rounded_selection,
            crate::state::ToggleAction::SelectionStyle,
        );
        draw_value(
            f,
            geo,
            &mut y,
            "A",
            "Font",
            match app.font_face.as_str() {
                "Cascadia Code" => "Cascadia".to_string(),
                "Lucida Console" => "Lucida".to_string(),
                "Nirmala UI" => "Tamil".to_string(),
                face => face.to_string(),
            },
            crate::state::ToggleAction::FontFamily,
        );
        draw_value(
            f,
            geo,
            &mut y,
            "A",
            "Size",
            format!("{}pt", app.font_size),
            crate::state::ToggleAction::FontSize,
        );
        draw_value(
            f,
            geo,
            &mut y,
            "B",
            "Weight",
            app.font_weight.to_string(),
            crate::state::ToggleAction::FontWeight,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "◉",
            "Hover",
            "Off",
            "On",
            app.hover_enabled,
            crate::state::ToggleAction::Hover,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "✏️",
            "Edit",
            "Off",
            "On",
            app.edit_preview_mode,
            crate::state::ToggleAction::EditMode,
        );
        draw_toggle(
            f,
            geo,
            &mut y,
            "📂",
            "DirClick",
            "Off",
            "On",
            app.dir_preview_clickable,
            crate::state::ToggleAction::DirPreviewClick,
        );
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
    if *y >= max_y {
        return;
    }
    let rect = Rect {
        x: inner.x,
        y: *y,
        width: inner.width,
        height: 1,
    };
    f.render_widget(Paragraph::new(content), rect);
    if let Some(p) = click_path {
        geo.sidebar_item_rects.push((rect, p));
    }
    *y += 1;
}

fn truncate(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let ellipsis = "…";
    let content_width = max.saturating_sub(UnicodeWidthStr::width(ellipsis));
    let mut used = 0usize;
    let mut out = String::new();
    for grapheme in s.graphemes(true) {
        let width = UnicodeWidthStr::width(grapheme);
        if used.saturating_add(width) > content_width {
            break;
        }
        out.push_str(grapheme);
        used += width;
    }
    out.push_str(ellipsis);
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
    let t = app.theme();
    let C_ACCENT = t.accent;
    let C_ACCENT2 = t.accent2;
    let C_TEXT = t.text;

    let title = match app.mode {
        AppMode::Rename => " Rename ",
        AppMode::NewFile => " New File ",
        _ => " New Folder ",
    };
    let term_size = f.size();

    let width = 64.min(term_size.width.saturating_sub(2)).max(1);
    let height = 3.min(term_size.height).max(1);

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
        .style(Style::default().bg(t.bg_panel));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let graphemes: Vec<&str> = app.input_buffer.graphemes(true).collect();
    let (lo, hi) = if app.input_sel_start <= app.input_cursor {
        (app.input_sel_start, app.input_cursor)
    } else {
        (app.input_cursor, app.input_sel_start)
    };

    let mut spans = Vec::new();
    for (i, grapheme) in graphemes.iter().enumerate() {
        let selected = i >= lo && i < hi;
        let style = if selected {
            Style::default().fg(Color::Black).bg(C_ACCENT2)
        } else {
            Style::default().fg(C_TEXT)
        };
        spans.push(Span::styled((*grapheme).to_string(), style));
    }
    // Cursor caret (only when there's no active selection to show).
    if lo == hi {
        let cursor_pos = app.input_cursor.min(spans.len());
        spans.insert(
            cursor_pos,
            Span::styled(
                "│",
                Style::default()
                    .fg(C_ACCENT)
                    .add_modifier(Modifier::RAPID_BLINK),
            ),
        );
    }

    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_confirm_modal(f: &mut Frame, app: &AppState) {
    let t = app.theme();
    let C_TEXT = t.text;
    let C_MUTED = t.muted;
    let C_ERR = t.err;

    let permanent = app.mode == AppMode::ConfirmDeletePermanent;
    let (count, targets) = app.delete_target_summary();
    let title = if permanent {
        " Permanently Delete "
    } else {
        " Delete "
    };
    let msg = if permanent {
        format!("Permanently delete {count} item(s)? This cannot be undone.")
    } else {
        format!("Move {count} item(s) to the Recycle Bin?")
    };

    let term_size = f.size();
    let width = 68.min(term_size.width.saturating_sub(2)).max(1);
    let preferred_height = 5u16.saturating_add(targets.len() as u16);
    let height = preferred_height.min(term_size.height).max(1);
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
        .border_style(Style::default().fg(C_ERR))
        .style(Style::default().bg(t.bg_panel));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let available_width = inner.width.saturating_sub(2) as usize;
    let mut text = vec![Line::from(Span::styled(
        truncate(&msg, available_width),
        Style::default().fg(C_TEXT),
    ))];
    text.extend(targets.into_iter().map(|target| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate(&target, available_width.saturating_sub(2)),
                Style::default().fg(C_MUTED),
            ),
        ])
    }));
    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled(" Y ", Style::default().fg(Color::Black).bg(C_ERR)),
        Span::raw(" confirm     "),
        Span::styled(" Esc ", Style::default().fg(Color::Black).bg(C_MUTED)),
        Span::raw(" cancel"),
    ]));
    f.render_widget(Paragraph::new(text), inner);
}

fn draw_properties_modal(f: &mut Frame, app: &AppState) {
    let t = app.theme();
    let C_ACCENT = t.accent;
    let C_TEXT = t.text;
    let C_MUTED = t.muted;
    let C_WARN = t.warn;
    let C_ERR = t.err;

    let Some(path) = app.properties_path.as_ref() else {
        return;
    };

    let term_size = f.size();
    let width = 78.min(term_size.width.saturating_sub(2)).max(1);
    let height = 18.min(term_size.height.saturating_sub(2)).max(1);
    let area = Rect {
        x: term_size.x + (term_size.width.saturating_sub(width)) / 2,
        y: term_size.y + (term_size.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(" Properties ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(t.bg_panel));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let meta = std::fs::metadata(path);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Name:      ", Style::default().fg(C_MUTED)),
            Span::styled(
                name,
                Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Location:  ", Style::default().fg(C_MUTED)),
            Span::styled(path.display().to_string(), Style::default().fg(C_TEXT)),
        ]),
    ];
    match &meta {
        Ok(m) => {
            let kind = if m.is_dir() {
                "Folder".to_string()
            } else {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .filter(|extension| !extension.is_empty())
                    .map(|extension| format!("{} file", extension.to_uppercase()))
                    .unwrap_or_else(|| "File".to_string())
            };
            lines.push(Line::from(vec![
                Span::styled("Type:      ", Style::default().fg(C_MUTED)),
                Span::styled(kind, Style::default().fg(C_TEXT)),
            ]));
            if m.is_dir() {
                if let Some(stats) = app.properties_stats.as_ref() {
                    let suffix = if stats.complete {
                        ""
                    } else {
                        "  (calculating...)"
                    };
                    lines.push(Line::from(vec![
                        Span::styled("Size:      ", Style::default().fg(C_MUTED)),
                        Span::styled(
                            format!(
                                "{} ({} bytes){}",
                                preview::human_size(stats.bytes),
                                stats.bytes,
                                suffix
                            ),
                            Style::default().fg(C_TEXT),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("Contains:  ", Style::default().fg(C_MUTED)),
                        Span::styled(
                            format!("{} files, {} folders", stats.files, stats.folders),
                            Style::default().fg(C_TEXT),
                        ),
                    ]));
                    if stats.inaccessible > 0 {
                        lines.push(Line::from(vec![
                            Span::styled("Skipped:   ", Style::default().fg(C_MUTED)),
                            Span::styled(
                                format!("{} inaccessible entries or links", stats.inaccessible),
                                Style::default().fg(C_WARN),
                            ),
                        ]));
                    }
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("Size:      ", Style::default().fg(C_MUTED)),
                        Span::styled("Calculating...", Style::default().fg(C_WARN)),
                    ]));
                }
            } else {
                lines.push(Line::from(vec![
                    Span::styled("Size:      ", Style::default().fg(C_MUTED)),
                    Span::styled(
                        format!("{} ({} bytes)", preview::human_size(m.len()), m.len()),
                        Style::default().fg(C_TEXT),
                    ),
                ]));
            }
            if let Ok(modified) = m.modified() {
                lines.push(property_time_line("Modified:  ", modified, C_MUTED, C_TEXT));
            }
            if let Ok(created) = m.created() {
                lines.push(property_time_line("Created:   ", created, C_MUTED, C_TEXT));
            }
            if let Ok(accessed) = m.accessed() {
                lines.push(property_time_line("Accessed:  ", accessed, C_MUTED, C_TEXT));
            }
            let mut attrs = Vec::new();
            attrs.push(if m.permissions().readonly() {
                "Read-only"
            } else {
                "Writable"
            });
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                let a = m.file_attributes();
                if a & 0x2 != 0 {
                    attrs.push("Hidden");
                }
                if a & 0x4 != 0 {
                    attrs.push("System");
                }
                if a & 0x20 != 0 {
                    attrs.push("Archive");
                }
                if a & 0x800 != 0 {
                    attrs.push("Compressed");
                }
                if a & 0x4000 != 0 {
                    attrs.push("Encrypted");
                }
            }
            lines.push(Line::from(vec![
                Span::styled("Attributes:", Style::default().fg(C_MUTED)),
                Span::styled(
                    format!(" {}", attrs.join(", ")),
                    Style::default().fg(C_WARN),
                ),
            ]));
            if let Ok(target) = std::fs::read_link(path) {
                lines.push(Line::from(vec![
                    Span::styled("Link to:   ", Style::default().fg(C_MUTED)),
                    Span::styled(target.display().to_string(), Style::default().fg(C_TEXT)),
                ]));
            }
        }
        Err(e) => {
            lines.push(Line::from(vec![
                Span::styled("Error:     ", Style::default().fg(C_ERR)),
                Span::styled(e.to_string(), Style::default().fg(C_ERR)),
            ]));
        }
    }
    lines.push(Line::from(""));
    if meta.as_ref().is_ok_and(|metadata| metadata.is_dir())
        && app
            .properties_stats
            .as_ref()
            .is_some_and(|stats| !stats.complete)
    {
        lines.push(Line::from(Span::styled(
            "Folder totals are calculated in the background.",
            Style::default().fg(C_MUTED),
        )));
    }
    lines.push(Line::from(Span::styled(
        "Esc to close",
        Style::default().fg(C_MUTED),
    )));

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn property_time_line(
    label: &'static str,
    time: std::time::SystemTime,
    label_color: Color,
    value_color: Color,
) -> Line<'static> {
    let local: chrono::DateTime<chrono::Local> = time.into();
    Line::from(vec![
        Span::styled(label, Style::default().fg(label_color)),
        Span::styled(
            local.format("%Y-%m-%d %H:%M:%S").to_string(),
            Style::default().fg(value_color),
        ),
    ])
}

fn compact_modified(modified: Option<std::time::SystemTime>) -> String {
    let Some(modified) = modified else {
        return "—".to_string();
    };
    let local: chrono::DateTime<chrono::Local> = modified.into();
    local.format("%m-%d %H:%M").to_string()
}

fn compact_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for candidate in UNITS {
        unit = candidate;
        if value < 1024.0 || *candidate == "T" {
            break;
        }
        value /= 1024.0;
    }
    if unit == "B" {
        format!("{}{}", bytes, unit)
    } else if value >= 100.0 {
        format!("{:.0}{}", value, unit)
    } else {
        format!("{:.1}{}", value, unit)
    }
}

fn theme_preview_lines(mut lines: Vec<Line<'static>>, mode: ThemeMode) -> Vec<Line<'static>> {
    if mode != ThemeMode::Light {
        return lines;
    }
    for line in &mut lines {
        for span in &mut line.spans {
            let adjusted = match span.style.fg {
                Some(Color::Rgb(203, 166, 247)) => Some(Color::Rgb(122, 62, 157)), // keyword
                Some(Color::Rgb(166, 227, 161)) => Some(Color::Rgb(42, 115, 54)),  // string
                Some(Color::Rgb(108, 112, 134)) => Some(Color::Rgb(102, 110, 125)), // comment
                Some(Color::Rgb(250, 179, 135)) => Some(Color::Rgb(156, 82, 28)),  // number
                Some(Color::Rgb(137, 180, 250)) => Some(Color::Rgb(28, 88, 170)),  // operator
                Some(Color::Rgb(205, 214, 244)) => Some(Color::Rgb(38, 45, 58)),   // text
                Some(Color::Rgb(88, 91, 112)) => Some(Color::Rgb(105, 112, 128)),  // line number
                Some(Color::Rgb(249, 226, 175)) => Some(Color::Rgb(137, 96, 0)),   // builtin/type
                Some(Color::Rgb(116, 199, 236)) => Some(Color::Rgb(0, 105, 135)),  // function
                Some(Color::Rgb(r, g, b)) => {
                    let luminance = (u16::from(r) * 3 + u16::from(g) * 6 + u16::from(b)) / 10;
                    if luminance > 140 {
                        Some(Color::Rgb(
                            ((u16::from(r) * 45) / 100) as u8,
                            ((u16::from(g) * 45) / 100) as u8,
                            ((u16::from(b) * 45) / 100) as u8,
                        ))
                    } else {
                        Some(Color::Rgb(r, g, b))
                    }
                }
                Some(Color::White) => Some(Color::Black),
                Some(Color::Gray) => Some(Color::DarkGray),
                other => other,
            };
            span.style.fg = adjusted;
        }
    }
    lines
}

// ─────────────────────────────────────────────────────────────────────────────
// Right-click context menu
// ─────────────────────────────────────────────────────────────────────────────

fn draw_context_menu(f: &mut Frame, app: &AppState, geo: &mut LayoutGeometry) {
    let t = app.theme();
    let C_ACCENT = t.accent;
    let C_TEXT = t.text;
    let C_ERR = t.err;

    let items = &app.context_menu_items;
    if items.is_empty() {
        return;
    }

    let term = f.size();
    let width: u16 = items
        .iter()
        .map(|a| a.label().len() as u16)
        .max()
        .unwrap_or(10)
        + 6;
    let height = items.len() as u16 + 2;

    let (mx, my) = app.pending_menu_pos;
    let mut x = mx;
    let mut y = my.saturating_add(1);
    if x + width > term.width {
        x = term.width.saturating_sub(width);
    }
    if y + height > term.height {
        y = term.height.saturating_sub(height);
    }

    let area = Rect {
        x,
        y,
        width: width.min(term.width),
        height: height.min(term.height),
    };
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_ACCENT))
        .style(Style::default().bg(t.bg_panel));
    let inner = block.inner(area);
    f.render_widget(block, area);

    for (i, action) in items.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let rect = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        let is_hovered = app.context_menu_hover == Some(i);

        let base_fg = if matches!(action, ContextAction::Delete) {
            C_ERR
        } else {
            C_TEXT
        };
        let (fg, bg) = if is_hovered {
            (Color::Black, C_ACCENT)
        } else {
            (base_fg, t.bg_panel)
        };

        let label_text = format!(" {}", action.label());
        let pad = " ".repeat(
            (inner.width as usize).saturating_sub(UnicodeWidthStr::width(label_text.as_str())),
        );
        let style = if is_hovered {
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg).bg(bg)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{}{}", label_text, pad),
                style,
            ))),
            rect,
        );
        geo.context_menu_item_rects.push((rect, *action));
    }
    geo.context_menu_rect = Some(area);
}

// ─────────────────────────────────────────────────────────────────────────────
// Breadcrumb — clickable path segments
// ─────────────────────────────────────────────────────────────────────────────

fn draw_breadcrumb(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    let t = app.theme();
    let C_ACCENT = t.accent;
    let C_TEXT = t.text;
    let C_MUTED = t.muted;
    let C_WARN = t.warn;

    let path = app.current().path.clone();
    let path_str = path.display().to_string();

    if path_str == "\\\\drives" {
        f.render_widget(
            Paragraph::new(Span::styled(
                " This PC — Drives",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            )),
            area,
        );
        return;
    }

    // Build clickable segments: C:\ > Users > sifay > Pictures
    let mut spans = vec![Span::styled("  ", Style::default().fg(C_MUTED))];
    let mut segments: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut acc = std::path::PathBuf::new();
    use std::path::Component;

    for comp in path.components() {
        acc.push(comp.as_os_str());

        if matches!(comp, Component::RootDir) {
            continue;
        }

        let label = comp.as_os_str().to_string_lossy().to_string();
        segments.push((label, acc.clone()));
    }

    for (label, _) in &mut segments {
        let trimmed = label.trim_end_matches(['\\', '/']);
        if !trimmed.is_empty() {
            *label = trimmed.to_string();
        }
    }
    let search_width = area.width.min(if app.search_active { 42 } else { 18 });
    let available = area.width.saturating_sub(search_width).saturating_sub(3) as usize;
    let mut hidden_middle = false;
    while segments.len() > 3
        && (segments
            .iter()
            .map(|(label, _)| UnicodeWidthStr::width(label.as_str()))
            .sum::<usize>()
            + segments.len().saturating_sub(1) * 3
            + usize::from(hidden_middle) * 4)
            > available
    {
        segments.remove(1);
        hidden_middle = true;
    }
    while segments.len() > 1
        && (segments
            .iter()
            .map(|(label, _)| UnicodeWidthStr::width(label.as_str()))
            .sum::<usize>()
            + segments.len().saturating_sub(1) * 3
            + usize::from(hidden_middle) * 4)
            > available
    {
        segments.remove(0);
        hidden_middle = true;
    }

    let mut x = area.x + 2;
    for (i, (label, seg_path)) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let style = if is_last {
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_TEXT)
        };
        if i > 0 {
            let separator = if i == 1 && hidden_middle {
                " … > "
            } else {
                " > "
            };
            spans.push(Span::styled(separator, Style::default().fg(C_MUTED)));
            x += UnicodeWidthStr::width(separator) as u16;
        }
        let text = label.clone();
        let w = UnicodeWidthStr::width(text.as_str()) as u16;
        if x + w <= area.x + area.width {
            geo.breadcrumb_segment_rects.push((
                Rect {
                    x,
                    y: area.y,
                    width: w,
                    height: 1,
                },
                seg_path.clone(),
            ));
        }
        spans.push(Span::styled(text, style));
        x += w;
    }

    let mode_badge = match app.mode {
        AppMode::Rename => "  [RENAME]",
        AppMode::ConfirmDelete | AppMode::ConfirmDeletePermanent => "  [DELETE?]",
        AppMode::NewFolder => "  [NEW FOLDER]",
        AppMode::NewFile => "  [NEW FILE]",
        AppMode::ContextMenu => "  [MENU]",
        AppMode::Properties => "  [PROPERTIES]",
        _ => "",
    };
    if !mode_badge.is_empty() {
        spans.push(Span::styled(
            mode_badge,
            Style::default().fg(C_WARN).add_modifier(Modifier::BOLD),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);

    if area.width > 0 {
        let (position, count) = app.search_match_position();
        let box_width = area.width.min(if app.search_active { 42 } else { 18 });
        let query_width = box_width.saturating_sub(14) as usize;
        let query = truncate(&app.search_query, query_width);
        let text = if !app.search_active {
            " /  Find files ".to_string()
        } else if app.search_query.is_empty() {
            " / Find files... ".to_string()
        } else {
            format!(" / {}  {}/{} ", query, position, count)
        };
        let search_rect = Rect {
            x: area.x + area.width.saturating_sub(box_width),
            y: area.y,
            width: box_width,
            height: 1,
        };
        geo.search_rect = Some(search_rect);
        f.render_widget(Clear, search_rect);
        f.render_widget(
            Paragraph::new(text).style(
                Style::default()
                    .fg(t.text)
                    .bg(t.sel_bg_inactive)
                    .add_modifier(if app.search_active {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            search_rect,
        );
        if app.search_active {
            let cursor_x = search_rect
                .x
                .saturating_add(3)
                .saturating_add(UnicodeWidthStr::width(query.as_str()) as u16)
                .min(
                    search_rect
                        .x
                        .saturating_add(search_rect.width.saturating_sub(1)),
                );
            f.set_cursor(cursor_x, search_rect.y);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-pane layout
// ─────────────────────────────────────────────────────────────────────────────

fn draw_panes(f: &mut Frame, app: &AppState, area: Rect, geo: &mut LayoutGeometry) {
    if app.layout_mode == LayoutMode::Explorer {
        let level_index = app.current_level.min(app.levels.len().saturating_sub(1));
        let Some(level) = app.levels.get(level_index) else {
            return;
        };
        let show_preview = !level.files.is_empty() && area.width >= 80;
        let chunks = if show_preview {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(72), Constraint::Min(0)])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0)])
                .split(area)
        };

        geo.pane_rects.push(chunks[0]);
        geo.pane_level_indices.insert(0, level_index);
        draw_tile_pane(f, app, level, chunks[0], geo, 0);
        if show_preview {
            geo.preview_rect = Some(chunks[1]);
            draw_preview_pane(f, app, level, chunks[1], geo);
        }
        return;
    }

    let num = app.levels.len();
    let maximum_columns = match area.width {
        0..=55 => 1usize,
        56..=83 => 2,
        84..=114 => 3,
        115..=149 => 4,
        _ => 5,
    };
    let wants_preview = app
        .levels
        .last()
        .is_some_and(|level| !level.files.is_empty())
        && maximum_columns >= 2;
    let pane_capacity = maximum_columns
        .saturating_sub(usize::from(wants_preview))
        .max(1);
    let visible_panes = num.min(pane_capacity).min(4);
    let start = num.saturating_sub(visible_panes);
    let panes = &app.levels[start..];
    let np = panes.len();

    let has_preview = wants_preview && panes.last().is_some_and(|level| !level.files.is_empty());
    let n_cols = if has_preview { np + 1 } else { np };

    if n_cols == 0 {
        return;
    }

    let start_idx = 5 - n_cols;
    let mut sub_ratios = app.column_ratios[start_idx..].to_vec();
    let sum: f32 = sub_ratios.iter().sum();
    if sum > 0.0 {
        for r in sub_ratios.iter_mut() {
            *r /= sum;
        }
    }

    let mut constraints: Vec<Constraint> = sub_ratios
        .iter()
        .take(n_cols.saturating_sub(1))
        .map(|&r| Constraint::Percentage((r * 100.0) as u16))
        .collect();
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
            geo.divider_rects.push(Rect {
                x: divider_x,
                y: current_chunk.y,
                width: 1,
                height: current_chunk.height,
            });
        }
    }

    for (i, level) in panes.iter().enumerate() {
        geo.pane_rects.push(chunks[i]);
        geo.pane_level_indices.insert(i, start + i);
        draw_dir_pane(
            f,
            app,
            level,
            (start + i) == app.current_level,
            chunks[i],
            geo,
            (start + i) - start,
        );
    }

    if has_preview {
        if let Some(current) = panes.last() {
            geo.preview_rect = Some(chunks[np]);
            draw_preview_pane(f, app, current, chunks[np], geo);
        }
    }
}

/// Windows-style Tiles view: items flow left-to-right in a responsive grid,
/// with an icon, filename, type, size, and modified date in each tile. The
/// whole tile is the hit target, so hover and selection remain immediate.
fn tile_grid_dimensions(width: u16, height: u16) -> (usize, usize, usize, u16) {
    const IDEAL_TILE_WIDTH: u16 = 30;
    const TILE_HEIGHT: u16 = 5;
    let columns = (width / IDEAL_TILE_WIDTH).max(1) as usize;
    let rows = (height / TILE_HEIGHT).max(1) as usize;
    let visible = columns.saturating_mul(rows);
    let tile_width = (width / columns as u16).max(1);
    (columns, rows, visible, tile_width)
}

fn draw_tile_pane(
    f: &mut Frame,
    app: &AppState,
    level: &DirLevel,
    area: Rect,
    geo: &mut LayoutGeometry,
    pane_idx: usize,
) {
    let t = app.theme();
    let title = level
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| level.path.display().to_string());
    let filtered_indices = level
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (!app.search_active
                || app.search_query.is_empty()
                || crate::state::filename_matches(entry, &app.search_query))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let title_text = format!(
        " {} · Tiles · {} {} · {} items ",
        if title.is_empty() { "/" } else { &title },
        app.sort_mode.label(),
        if app.sort_descending { "↓" } else { "↑" },
        filtered_indices.len(),
    );
    let block = Block::default()
        .title(title_text.clone())
        .title_style(Style::default().fg(t.accent).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.bg_panel));
    let inner = block.inner(area);
    f.render_widget(block, area);

    const TILE_HEIGHT: u16 = 5;
    let (columns, _rows, visible_items, tile_width) =
        tile_grid_dimensions(inner.width, inner.height);
    geo.tile_columns = columns;
    geo.tile_visible_items = visible_items;

    let selected_position = filtered_indices
        .iter()
        .position(|index| *index == level.selected)
        .unwrap_or(0);
    let start = if app.search_active && !app.search_query.is_empty() {
        selected_position
            .saturating_sub(visible_items.saturating_sub(columns) / 2)
            .div_euclid(columns)
            * columns
    } else {
        level
            .scroll
            .min(filtered_indices.len().saturating_sub(visible_items))
            .div_euclid(columns)
            * columns
    };
    let mut hitboxes = Vec::new();

    for (slot, file_index) in filtered_indices
        .iter()
        .skip(start)
        .take(visible_items)
        .enumerate()
    {
        let file_index = *file_index;
        let entry = &level.files[file_index];
        let column = slot % columns;
        let row = slot / columns;
        let x = inner.x + column as u16 * tile_width;
        let right = if column + 1 == columns {
            inner.x + inner.width
        } else {
            x + tile_width
        };
        let rect = Rect {
            x,
            y: inner.y + row as u16 * TILE_HEIGHT,
            width: right.saturating_sub(x),
            height: TILE_HEIGHT.min(inner.y + inner.height - (inner.y + row as u16 * TILE_HEIGHT)),
        };
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        hitboxes.push((file_index, rect));

        let is_selected = file_index == level.selected;
        let is_hovered = app.hovered_row == Some((pane_idx, file_index));
        let is_marked = level.marked.contains(&entry.path);
        let background = if is_selected {
            t.sel_bg
        } else if is_hovered {
            t.sel_bg_inactive
        } else {
            t.bg_panel
        };
        let border_color = if is_selected {
            t.accent
        } else if is_hovered {
            t.accent2
        } else {
            t.bg_panel
        };
        let tile_block = Block::default()
            .borders(Borders::ALL)
            .border_type(if app.rounded_selection {
                BorderType::Rounded
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color).bg(background))
            .style(Style::default().bg(background));
        let tile_inner = tile_block.inner(rect);
        f.render_widget(tile_block, rect);

        let name = entry
            .path
            .file_name()
            .unwrap_or_else(|| entry.path.as_os_str())
            .to_string_lossy();
        let extension = entry
            .path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let (icon, icon_color) = if entry.is_dir {
            (get_icon_set().folder, t.folder)
        } else {
            file_icon(extension)
        };
        let type_label = if entry.is_dir {
            "File folder".to_string()
        } else if extension.is_empty() {
            "File".to_string()
        } else {
            format!("{} file", extension.to_ascii_uppercase())
        };
        let details = if entry.is_dir {
            compact_modified(entry.modified)
        } else {
            format!(
                "{} · {}",
                compact_size(entry.size),
                compact_modified(entry.modified)
            )
        };
        let name_budget = usize::from(tile_inner.width).saturating_sub(4).max(4);
        let marker = if is_marked { "✓ " } else { "" };
        let lines = vec![
            Line::from(vec![
                Span::styled(icon, Style::default().fg(icon_color).bg(background)),
                Span::raw("  "),
                Span::styled(
                    format!("{}{}", marker, truncate(&name, name_budget)),
                    Style::default()
                        .fg(if is_selected { t.accent } else { t.text })
                        .bg(background)
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ]),
            Line::from(Span::styled(
                format!("    {}", truncate(&type_label, name_budget)),
                Style::default().fg(t.text_soft).bg(background),
            )),
            Line::from(Span::styled(
                format!("    {}", truncate(&details, name_budget)),
                Style::default().fg(t.muted).bg(background),
            )),
        ];
        f.render_widget(Paragraph::new(lines), tile_inner);
    }
    geo.row_rects.insert(pane_idx, hitboxes);
    geo.pane_sort_rect = Some(Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: UnicodeWidthStr::width(title_text.as_str())
            .min(area.width.saturating_sub(2) as usize) as u16,
        height: 1,
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Directory column pane
// ─────────────────────────────────────────────────────────────────────────────

fn draw_dir_pane(
    f: &mut Frame,
    app: &AppState,
    level: &DirLevel,
    is_current: bool,
    area: Rect,
    geo: &mut LayoutGeometry,
    pane_idx: usize,
) {
    let t = app.theme();
    let C_ACCENT = t.accent;
    let C_ACCENT2 = t.accent2;
    let C_BORDER = t.border;
    let C_TEXT = t.text;
    let C_MUTED = t.muted;
    let C_SEL_BG = t.sel_bg;
    let C_SEL_BG_INACTIVE = t.sel_bg_inactive;
    let C_MARK = t.mark;
    let C_FOLDER = t.folder;

    let path_str = level.path.display().to_string();
    let title = if path_str == "\\\\drives" {
        "Drives".to_string()
    } else {
        level
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.trim_end_matches(['/', '\\']).to_string())
    };

    let border_style = if is_current {
        Style::default().fg(C_ACCENT)
    } else {
        Style::default().fg(C_BORDER)
    };

    let visible_h = area.height.saturating_sub(2) as usize;
    let filtered_indices: Vec<usize> =
        if is_current && app.search_active && !app.search_query.is_empty() {
            level
                .files
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    crate::state::filename_matches(entry, &app.search_query).then_some(index)
                })
                .collect()
        } else {
            (0..level.files.len()).collect()
        };
    let filtered_selected = filtered_indices
        .iter()
        .position(|index| *index == level.selected)
        .unwrap_or(0);
    let scroll_start = if is_current && app.search_active && !app.search_query.is_empty() {
        filtered_selected
            .saturating_sub(visible_h.saturating_sub(1) / 2)
            .min(filtered_indices.len().saturating_sub(visible_h))
    } else {
        level
            .scroll
            .min(filtered_indices.len().saturating_sub(visible_h))
    };

    let mut row_rects_for_pane = Vec::new();
    let list_inner_area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let items: Vec<ListItem> = filtered_indices
        .iter()
        .skip(scroll_start)
        .take(visible_h)
        .enumerate()
        .map(|(render_i, file_i)| {
            let file_i = *file_i;
            let p = &level.files[file_i];
            row_rects_for_pane.push((
                file_i,
                Rect {
                    x: list_inner_area.x,
                    y: list_inner_area.y + render_i as u16,
                    width: list_inner_area.width,
                    height: 1,
                },
            ));

            let raw_name = p
                .path
                .file_name()
                .unwrap_or_else(|| p.path.as_os_str())
                .to_string_lossy()
                .to_string();
            let details = if app.show_file_details || app.layout_mode == LayoutMode::Explorer {
                let size = if p.is_dir {
                    String::new()
                } else {
                    compact_size(p.size)
                };
                if list_inner_area.width >= 29 {
                    format!("{:>6} {:>11}", size, compact_modified(p.modified))
                } else if list_inner_area.width >= 20 {
                    format!("{:>7}", size)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let details_width = Span::raw(details.as_str()).width();
            let name_budget = (list_inner_area.width as usize)
                .saturating_sub(3 + details_width + usize::from(!details.is_empty()));
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
            let is_hovered = app.hovered_row == Some((pane_idx, file_i));
            let is_search_match = is_current
                && app.search_active
                && !app.search_query.is_empty()
                && crate::state::filename_matches(p, &app.search_query);

            let icon_span = Span::styled(icon, Style::default().fg(icon_color));

            if is_selected {
                let bg = if is_current {
                    C_SEL_BG
                } else {
                    C_SEL_BG_INACTIVE
                };
                let fg_color = if is_current { C_ACCENT } else { C_TEXT };
                let inner_w = (list_inner_area.width as usize).saturating_sub(2);

                let icon_selected_span = Span::styled(icon, Style::default().fg(icon_color).bg(bg));
                let name_selected_span = Span::styled(
                    shown_name.clone(),
                    Style::default()
                        .fg(fg_color)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                );

                let disp_w = icon_selected_span.width()
                    + 1
                    + name_selected_span.width()
                    + details_width
                    + usize::from(!details.is_empty());
                let pad_w = inner_w.saturating_sub(disp_w);

                let mut spans = Vec::new();
                if app.rounded_selection {
                    spans.push(Span::styled(
                        PILL_LEFT_CAP,
                        Style::default().fg(bg).bg(t.bg_panel),
                    ));
                } else {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                spans.extend([
                    icon_selected_span,
                    Span::styled(" ", Style::default().bg(bg)),
                    name_selected_span,
                    Span::styled(" ".repeat(pad_w), Style::default().bg(bg)),
                ]);
                if !details.is_empty() {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                    spans.push(Span::styled(details, Style::default().fg(C_MUTED).bg(bg)));
                }
                if app.rounded_selection {
                    spans.push(Span::styled(
                        PILL_RIGHT_CAP,
                        Style::default().fg(bg).bg(t.bg_panel),
                    ));
                } else {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                ListItem::new(Line::from(spans))
            } else if is_hovered {
                let bg = t.sel_bg_inactive;
                let inner_w = (list_inner_area.width as usize).saturating_sub(2);
                let used = icon_span.width()
                    + 1
                    + Span::raw(shown_name.as_str()).width()
                    + details_width
                    + usize::from(!details.is_empty());
                let mut spans = vec![
                    if app.rounded_selection {
                        Span::styled(PILL_LEFT_CAP, Style::default().fg(bg).bg(t.bg_panel))
                    } else {
                        Span::styled(" ", Style::default().bg(bg))
                    },
                    Span::styled(icon, Style::default().fg(icon_color).bg(bg)),
                    Span::styled(" ", Style::default().bg(bg)),
                    Span::styled(
                        shown_name.clone(),
                        Style::default()
                            .fg(C_ACCENT)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " ".repeat(inner_w.saturating_sub(used)),
                        Style::default().bg(bg),
                    ),
                ];
                if !details.is_empty() {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                    spans.push(Span::styled(details, Style::default().fg(C_MUTED).bg(bg)));
                }
                spans.push(if app.rounded_selection {
                    Span::styled(PILL_RIGHT_CAP, Style::default().fg(bg).bg(t.bg_panel))
                } else {
                    Span::styled(" ", Style::default().bg(bg))
                });
                ListItem::new(Line::from(spans))
            } else if is_marked {
                let used = 1
                    + icon_span.width()
                    + 1
                    + Span::raw(shown_name.as_str()).width()
                    + details_width
                    + usize::from(!details.is_empty());
                let mut spans = vec![
                    Span::raw(" "),
                    icon_span,
                    Span::styled(" ", Style::default()),
                    Span::styled(
                        shown_name.clone(),
                        Style::default().fg(C_MARK).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" ".repeat((list_inner_area.width as usize).saturating_sub(used))),
                ];
                if !details.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(details, Style::default().fg(C_MUTED)));
                }
                ListItem::new(Line::from(spans))
            } else {
                let style = if is_search_match {
                    Style::default()
                        .fg(C_ACCENT)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else if is_current {
                    Style::default().fg(C_TEXT)
                } else {
                    Style::default().fg(C_MUTED)
                };
                let used = 1
                    + icon_span.width()
                    + 1
                    + Span::raw(shown_name.as_str()).width()
                    + details_width
                    + usize::from(!details.is_empty());
                let mut spans = vec![
                    Span::raw(" "),
                    icon_span,
                    Span::styled(" ", Style::default()),
                    Span::styled(shown_name.clone(), style),
                    Span::raw(" ".repeat((list_inner_area.width as usize).saturating_sub(used))),
                ];
                if !details.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(details, Style::default().fg(C_MUTED)));
                }
                ListItem::new(Line::from(spans))
            }
        })
        .collect();

    geo.row_rects.insert(pane_idx, row_rects_for_pane);

    let marked_count = level.marked.len();
    let title_text = if is_current && app.search_active && !app.search_query.is_empty() {
        format!(
            " {} · {} matches · {} {} ",
            if title.is_empty() { "/" } else { &title },
            filtered_indices.len(),
            app.sort_mode.label(),
            if app.sort_descending { "↓" } else { "↑" }
        )
    } else if marked_count > 0 {
        format!(
            " {} · {} marked ",
            if title.is_empty() { "/" } else { &title },
            marked_count
        )
    } else if app.layout_mode == LayoutMode::Explorer {
        format!(
            " {} · {} {} · Size · Modified ",
            if title.is_empty() { "/" } else { &title },
            app.sort_mode.label(),
            if app.sort_descending { "↓" } else { "↑" },
        )
    } else {
        format!(
            " {} · {}{} ",
            if title.is_empty() { "/" } else { &title },
            app.sort_mode.label(),
            if app.sort_descending { " ↓" } else { " ↑" },
        )
    };

    let title_width = UnicodeWidthStr::width(title_text.as_str()).min(area.width as usize) as u16;
    let list = List::new(items).block(
        Block::default()
            .title(title_text)
            .title_style(if is_current {
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(C_MUTED)
            })
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .style(Style::default().bg(t.bg_panel)),
    );
    f.render_widget(list, area);
    if is_current {
        geo.pane_sort_rect = Some(Rect {
            x: area.x.saturating_add(1),
            y: area.y,
            width: title_width.saturating_sub(1),
            height: 1,
        });
    }

    // Simple scrollbar indicator for long directories.
    if filtered_indices.len() > visible_h && area.height > 4 {
        let track_h = area.height.saturating_sub(2) as usize;
        let ratio = visible_h as f64 / filtered_indices.len() as f64;
        let thumb_h = ((track_h as f64) * ratio).max(1.0) as usize;
        let thumb_pos = if filtered_indices.len() > visible_h {
            ((track_h.saturating_sub(thumb_h)) as f64
                * (scroll_start as f64 / (filtered_indices.len() - visible_h) as f64))
                as usize
        } else {
            0
        };
        for i in 0..track_h {
            let on_thumb = i >= thumb_pos && i < thumb_pos + thumb_h;
            let ch = if on_thumb { "█" } else { "│" };
            let color = if on_thumb { C_ACCENT2 } else { C_BORDER };
            let rect = Rect {
                x: area.x + area.width - 1,
                y: area.y + 1 + i as u16,
                width: 1,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Span::styled(ch, Style::default().fg(color))),
                rect,
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preview pane
// ─────────────────────────────────────────────────────────────────────────────

fn draw_preview_pane(
    f: &mut Frame,
    app: &AppState,
    level: &DirLevel,
    area: Rect,
    geo: &mut LayoutGeometry,
) {
    let t = app.theme();
    let C_BORDER = t.border;
    let C_BORDER_LO = t.border_lo;
    let C_TEXT = t.text;
    let C_TEXT_SOFT = t.text_soft;
    let C_MUTED = t.muted;
    let C_WARN = t.warn;
    let C_OK = t.ok;
    let C_FOLDER = t.folder;

    // Replace every cell in the preview region before drawing new content.
    // This is important for complex scripts whose combining glyphs may
    // otherwise survive a shorter replacement line in the terminal buffer.
    f.render_widget(Clear, area);

    if level.files.is_empty() {
        app.native_preview.hide();
        f.render_widget(
            Paragraph::new(Span::styled("Empty", Style::default().fg(C_MUTED))).block(
                Block::default()
                    .title(" PREVIEW ")
                    .borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(C_BORDER))
                    .style(Style::default().bg(t.bg_panel)),
            ),
            area,
        );
        return;
    }

    let selected = &level.files[level.selected];

    // ── Directory preview ─────────────────────────────────────────────────────
    if selected.is_dir {
        let mut children = preview::cached_list_dir(&selected.path);
        crate::state::sort_dir_entries(&mut children, app.sort_mode, app.sort_descending);
        let dir_count = children.iter().filter(|p| p.is_dir).count();
        let file_count = children.len() - dir_count;
        let img_count = children
            .iter()
            .filter(|p| {
                let ext = p
                    .path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                matches!(
                    ext.as_str(),
                    "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tiff" | "tif"
                )
            })
            .count();

        let dir_name = selected
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let stats_title = if img_count > 0 {
            format!(
                " {} │ {} dirs  {} files  {} imgs (Clickable) ",
                dir_name, dir_count, file_count, img_count
            )
        } else {
            format!(
                " {} │ {} dirs  {} files (Clickable) ",
                dir_name, dir_count, file_count
            )
        };

        let visible_h = area.height.saturating_sub(2) as usize;
        let items: Vec<ListItem> = children
            .iter()
            .take(visible_h)
            .enumerate()
            .map(|(i, p)| {
                let raw = p
                    .path
                    .file_name()
                    .unwrap_or_else(|| p.path.as_os_str())
                    .to_string_lossy();
                let details = if app.show_file_details {
                    let size = if p.is_dir {
                        String::new()
                    } else {
                        compact_size(p.size)
                    };
                    if area.width >= 32 {
                        format!("{:>6} {:>11}", size, compact_modified(p.modified))
                    } else {
                        format!("{:>7}", size)
                    }
                } else {
                    String::new()
                };
                let details_width = Span::raw(details.as_str()).width();
                let name_budget = (area.width as usize)
                    .saturating_sub(4 + details_width + usize::from(!details.is_empty()));
                let name = truncate(&raw, name_budget.max(4));
                let folder_icon = get_icon_set().folder;
                let (icon, icon_color) = if p.is_dir {
                    (folder_icon, C_FOLDER)
                } else {
                    let ext = p.path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    file_icon(ext)
                };

                let row_rect = Rect {
                    x: area.x + 1,
                    y: area.y + 1 + i as u16,
                    width: area.width.saturating_sub(2),
                    height: 1,
                };
                geo.preview_dir_item_rects.push((row_rect, p.path.clone()));

                let used = Span::raw(icon).width()
                    + 1
                    + Span::raw(name.as_str()).width()
                    + details_width
                    + usize::from(!details.is_empty());
                let mut spans = vec![
                    Span::styled(icon, Style::default().fg(icon_color)),
                    Span::styled(" ", Style::default()),
                    Span::styled(name, Style::default().fg(C_TEXT_SOFT)),
                    Span::raw(
                        " ".repeat((area.width.saturating_sub(2) as usize).saturating_sub(used)),
                    ),
                ];
                if !details.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(details, Style::default().fg(C_MUTED)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        f.render_widget(
            List::new(items).block(
                Block::default()
                    .title(stats_title)
                    .title_style(Style::default().fg(C_OK))
                    .borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(C_BORDER_LO))
                    .style(Style::default().bg(t.bg_panel)),
            ),
            area,
        );
        app.native_preview.hide();
        return;
    }

    // ── File preview ──────────────────────────────────────────────────────────
    let name = selected
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = std::fs::metadata(&selected.path);
    let size_str = meta
        .as_ref()
        .map(|m| preview::human_size(m.len()))
        .unwrap_or_default();
    let ext = selected
        .path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_uppercase();

    // ── Interactive Preview Editor Mode ───────────────────────────────────────
    if app.edit_preview_mode {
        app.native_preview.hide();
        let title_text = if app.edit_dirty {
            format!(" EDIT MODE │ {} * ", name)
        } else {
            format!(" EDIT MODE │ {} ", name)
        };
        let title_color = if app.edit_dirty { C_WARN } else { C_OK };

        let block = Block::default()
            .title(title_text)
            .title_style(
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(title_color))
            .style(Style::default().bg(t.bg_panel));

        let inner = block.inner(area);
        f.render_widget(block, area);

        // Header action button bar
        let save_btn_text = if app.edit_dirty {
            " [ 💾 Save (Ctrl+S) ] "
        } else {
            " [ Saved ] "
        };
        let save_btn_style = if app.edit_dirty {
            Style::default()
                .fg(Color::Black)
                .bg(C_OK)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_MUTED)
        };
        let save_btn_span = Span::styled(save_btn_text, save_btn_style);
        let btn_w = save_btn_span.width() as u16;
        let save_rect = Rect {
            x: inner.x + inner.width.saturating_sub(btn_w + 1),
            y: inner.y,
            width: btn_w,
            height: 1,
        };
        geo.edit_save_btn_rect = Some(save_rect);

        let editor_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };

        // Keep the cursor visible vertically. The terminal cursor overlays the
        // text after rendering, so moving it never inserts a fake character or
        // shifts the remainder of the line.
        let visible_height = editor_area.height as usize;
        let mut scroll = app.preview_scroll;
        if app.edit_cursor_row < scroll {
            scroll = app.edit_cursor_row;
        } else if visible_height > 0 && app.edit_cursor_row >= scroll + visible_height {
            scroll = app.edit_cursor_row + 1 - visible_height;
        }

        let edit_ext = selected
            .path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_lowercase();
        let highlighted = theme_preview_lines(
            preview::highlight_editor_buffer(&app.edit_buffer, &edit_ext),
            app.theme_mode,
        );
        let lines = highlighted
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect::<Vec<_>>();

        f.render_widget(Paragraph::new(Text::from(lines)), editor_area);
        f.render_widget(Paragraph::new(Line::from(save_btn_span)), save_rect);

        if visible_height > 0
            && app.edit_cursor_row >= scroll
            && app.edit_cursor_row < scroll + visible_height
        {
            let raw_line = &app.edit_buffer[app.edit_cursor_row];
            let head = raw_line
                .graphemes(true)
                .take(app.edit_cursor_col.min(raw_line.graphemes(true).count()))
                .collect::<String>();
            let display_head = preview::expand_editor_tabs(&head);
            let gutter_digits = app.edit_buffer.len().max(1).to_string().len();
            let gutter_width = Span::raw(format!(
                "{:>width$}│",
                app.edit_cursor_row + 1,
                width = gutter_digits,
            ))
            .width() as u16;
            let cursor_x = editor_area
                .x
                .saturating_add(gutter_width)
                .saturating_add(Span::raw(display_head).width() as u16);
            let cursor_y = editor_area.y + (app.edit_cursor_row - scroll) as u16;
            if cursor_x < editor_area.x.saturating_add(editor_area.width) {
                f.set_cursor(cursor_x, cursor_y);
            }
        }
        return;
    }

    let title = if app.mode != AppMode::Normal {
        " PREVIEW │ mode active (Esc to cancel) ".to_string()
    } else {
        " PREVIEW ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(if app.mode != AppMode::Normal {
            Style::default().fg(C_WARN)
        } else {
            Style::default().fg(C_MUTED)
        })
        .borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(if app.mode != AppMode::Normal {
            Style::default().fg(C_WARN)
        } else {
            Style::default().fg(C_BORDER_LO)
        })
        .style(Style::default().bg(t.bg_panel));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let scroll = app.preview_scroll as u16;

    if app.preview_debounce_active() {
        app.native_preview.hide();
        f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", app.preview_mode.label().to_ascii_uppercase()),
                        Style::default()
                            .fg(Color::Black)
                            .bg(t.accent2)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  Settling selection…", Style::default().fg(C_MUTED)),
                ]),
                Line::from(Span::styled(
                    format!("  {name}"),
                    Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("  {}  ·  {}", ext, size_str),
                    Style::default().fg(C_TEXT_SOFT),
                )),
            ])),
            inner,
        );
        return;
    }

    let Some(preview_content) = app.prepared_preview() else {
        app.native_preview.hide();
        f.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(Span::styled(
                    "  Preparing preview…",
                    Style::default().fg(C_WARN).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("  {name}  ·  {ext}  ·  {size_str}"),
                    Style::default().fg(C_MUTED),
                )),
            ])),
            inner,
        );
        return;
    };

    match preview_content {
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
            for line in theme_preview_lines(lines.clone(), app.theme_mode) {
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
            // Code is line-addressable, so only style and draw the visible
            // viewport. The cache still retains every line for instant scroll.
            let start = (scroll as usize).min(lines.len());
            let end = start.saturating_add(inner.height as usize).min(lines.len());
            let visible = lines[start..end].to_vec();
            let para = Paragraph::new(Text::from(theme_preview_lines(visible, app.theme_mode)));
            f.render_widget(para, inner);
        }
        PreviewContent::Status(info) => {
            app.native_preview.hide();
            let (badge, color) = match info.kind {
                preview::PreviewStatusKind::Loading => ("RENDERING", C_WARN),
                preview::PreviewStatusKind::Fallback => ("FALLBACK", C_WARN),
                preview::PreviewStatusKind::Unsupported => ("UNSUPPORTED", C_MUTED),
                preview::PreviewStatusKind::Failed => ("FAILED", t.err),
                preview::PreviewStatusKind::Info => ("INFO", t.accent2),
            };
            let mut lines = vec![
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!(" {badge} "),
                        Style::default()
                            .fg(Color::Black)
                            .bg(color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        info.title.as_str(),
                        Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(info.detail.as_str(), Style::default().fg(C_TEXT_SOFT)),
                ]),
            ];
            if app.is_paged_visual_selected() {
                let (previous, next) = page_control_rects(inner);
                geo.slide_prev_rect = Some(previous);
                geo.slide_next_rect = Some(next);
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(PAGE_PREV_LABEL, Style::default().fg(t.accent2)),
                    Span::raw("  "),
                    Span::styled(PAGE_NEXT_LABEL, Style::default().fg(t.accent2)),
                    Span::styled(
                        format!(
                            "  ·  Page {}{}",
                            app.preview_page_index + 1,
                            app.preview_page_count
                                .map(|count| format!("/{count}"))
                                .unwrap_or_default()
                        ),
                        Style::default().fg(C_MUTED),
                    ),
                ]));
            }
            if let Some(renderer) = &info.renderer {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Renderer: ", Style::default().fg(C_MUTED)),
                    Span::styled(renderer, Style::default().fg(color)),
                ]));
            }
            if let Some(action) = &info.action {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(action, Style::default().fg(C_MUTED)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "─".repeat((inner.width as usize).saturating_sub(4)),
                    Style::default().fg(C_BORDER_LO),
                ),
            ]));
            for line in theme_preview_lines(info.lines.clone(), app.theme_mode) {
                let mut spans = line.spans;
                spans.insert(0, Span::raw("  "));
                lines.push(Line::from(spans));
            }
            let para = Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            f.render_widget(para, inner);
        }
        PreviewContent::ImageFallback(info) => {
            let mut top_margin = 0;
            let mut text = vec![Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    &name,
                    Style::default().fg(C_TEXT).add_modifier(Modifier::BOLD),
                ),
            ])];
            let mut meta_str = if matches!(ext.as_str(), "PPT" | "PPTX" | "ODP")
                && app.is_paged_visual_selected()
            {
                match app.preview_page_count {
                    Some(count) => format!(
                        "{} Slide {}/{}  |  {}",
                        ext,
                        app.preview_page_index.saturating_add(1).min(count),
                        count,
                        size_str,
                    ),
                    None => format!(
                        "{} Slide {}  |  {}",
                        ext,
                        app.preview_page_index + 1,
                        size_str
                    ),
                }
            } else if ext == "PDF" && app.is_paged_visual_selected() {
                match app.preview_page_count {
                    Some(count) => format!(
                        "PDF Page {}/{}  |  {}",
                        app.preview_page_index.saturating_add(1).min(count),
                        count,
                        size_str,
                    ),
                    None => format!("PDF Page {}  |  {}", app.preview_page_index + 1, size_str,),
                }
            } else {
                format!("{} Image  |  {}", ext, size_str)
            };
            if let Some((w, h)) = info.dimensions {
                meta_str.push_str(&format!("  |  {} x {}", w, h));
            }
            meta_str.push_str(&format!("  |  Zoom {:.0}%", app.image_zoom * 100.0));
            if !app.image_rotation.is_multiple_of(360) {
                meta_str.push_str(&format!("  |  Rotate {}°", app.image_rotation % 360));
            }
            if app.image_flip_h {
                meta_str.push_str("  |  Flipped");
            }
            if let Some(caption) = &info.caption {
                meta_str.push_str("  |  ");
                meta_str.push_str(caption);
            }
            text.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(meta_str, Style::default().fg(C_MUTED)),
            ]));
            let paged = matches!(ext.as_str(), "PDF" | "PPT" | "PPTX" | "ODP")
                && app.is_paged_visual_selected();
            if paged {
                let (previous, next) = page_control_rects(inner);
                geo.slide_prev_rect = Some(previous);
                geo.slide_next_rect = Some(next);
                text.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        PAGE_PREV_LABEL,
                        if app.preview_page_index > 0 {
                            Style::default().fg(t.accent2).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(C_MUTED)
                        },
                    ),
                    Span::raw("  "),
                    Span::styled(
                        PAGE_NEXT_LABEL,
                        if app
                            .preview_page_count
                            .is_none_or(|count| app.preview_page_index.saturating_add(1) < count)
                        {
                            Style::default().fg(t.accent2).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(C_MUTED)
                        },
                    ),
                    Span::styled(
                        "  ·  Wheel/Left/Right pages  ·  +/- zoom",
                        Style::default().fg(C_MUTED),
                    ),
                ]));
            } else {
                text.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "+/- zoom  ·  0 fit  ·  R rotate  ·  F flip",
                        Style::default().fg(C_MUTED),
                    ),
                ]));
            }
            text.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "─".repeat((inner.width as usize).saturating_sub(4)),
                    Style::default().fg(C_BORDER_LO),
                ),
            ]));

            if let Some(img) = &info.img {
                let available_width = usize::from(inner.width.max(1));
                let header_rows = text
                    .iter()
                    .map(|line| line.width().max(1).div_ceil(available_width))
                    .sum::<usize>()
                    .min(usize::from(inner.height.saturating_sub(1)))
                    as u16;
                let mut img_rect = inner;
                if img_rect.height > header_rows.saturating_add(2) {
                    img_rect.y = img_rect.y.saturating_add(header_rows);
                    img_rect.height = img_rect.height.saturating_sub(header_rows);
                }
                let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
                if app.mode == AppMode::Normal {
                    let background = match t.bg_panel {
                        Color::Rgb(r, g, b) => {
                            u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
                        }
                        Color::White => 0x00FF_FFFF,
                        _ => 30 | (30 << 8) | (46 << 16),
                    };
                    app.native_preview.show(
                        std::sync::Arc::clone(img),
                        info.path.clone(),
                        app.image_rotation,
                        app.image_flip_h,
                        app.image_zoom,
                        img_rect,
                        cols,
                        rows,
                        background,
                        app.preview_mode.is_blitz(),
                    );
                } else {
                    app.native_preview.hide();
                }
                top_margin = img_rect.y.saturating_sub(inner.y);
            } else {
                app.native_preview.hide();
            }

            for _ in 0..top_margin.saturating_sub(text.len() as u16) {
                text.push(Line::from(""));
            }

            let para = Paragraph::new(Text::from(text)).wrap(Wrap { trim: false });
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
        let val = std::env::var("RUSTY_RANGER_ICONS")
            .unwrap_or_default()
            .to_lowercase();
        if val == "nerd" || std::env::var("NERD_FONT").is_ok() {
            NERD_ICONS
        } else if val == "ascii" {
            ASCII_ICONS
        } else {
            // Rich icons are the default. Layout code reserves fixed icon
            // slots so fallback-font width cannot move adjacent labels.
            EMOJI_ICONS
        }
    });
    &SET
}

fn file_icon(ext: &str) -> (&'static str, Color) {
    const CODE: Color = Color::Rgb(137, 180, 250); // blue
    const MARKUP: Color = Color::Rgb(148, 226, 213); // teal
    const DOC: Color = Color::Rgb(203, 166, 247); // lavender
    const SHEET: Color = Color::Rgb(166, 227, 161); // green
    const IMAGE: Color = Color::Rgb(245, 194, 231); // pink
    const MEDIA: Color = Color::Rgb(250, 179, 135); // peach
    const ARCHIVE: Color = Color::Rgb(249, 226, 175); // yellow
    const BIN: Color = Color::Rgb(243, 139, 168); // red
    const PLAIN: Color = Color::Rgb(120, 126, 145);

    let set = get_icon_set();
    match ext.to_lowercase().as_str() {
        // Specific languages with their iconic brand colors
        "rs" => (set.rs, Color::Rgb(224, 112, 16)), // orange
        "py" | "pyw" => ("Py", Color::Rgb(55, 118, 171)), // Python's standard Windows blue
        "js" | "mjs" | "cjs" => (set.js, Color::Rgb(240, 219, 79)), // yellow
        "ts" | "tsx" | "jsx" => (set.ts, Color::Rgb(0, 122, 204)), // blue
        "html" | "htm" => (set.html, Color::Rgb(227, 76, 38)), // HTML red
        "css" | "scss" | "sass" | "less" => (set.css, Color::Rgb(86, 61, 124)), // CSS purple
        "go" => (set.go, Color::Rgb(0, 162, 232)),  // blue
        "c" | "cpp" | "h" | "hpp" | "cc" => (set.c, Color::Rgb(63, 81, 181)), // blue
        "java" | "kt" | "kts" => (set.java, Color::Rgb(244, 67, 54)), // red
        "rb" => (set.ruby, Color::Rgb(204, 0, 0)),  // red
        "php" => (set.php, Color::Rgb(119, 123, 179)), // blue
        "swift" => (set.swift, Color::Rgb(255, 102, 0)), // orange
        "cs" => (set.csharp, Color::Rgb(23, 150, 18)), // green
        "lua" => (set.lua, Color::Rgb(0, 0, 128)),  // blue
        "sql" => (set.sql, Color::Rgb(0, 150, 136)), // teal

        // Config / Markup / Text
        "json" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" => (set.config, CODE),
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => (set.shell, Color::Rgb(255, 235, 59)), // yellow
        "txt" | "log" => (set.text, PLAIN),
        "md" | "markdown" | "rst" | "rtf" => (set.md, MARKUP),

        // Office & PDFs
        "pdf" => (set.pdf, BIN),                                      // red
        "docx" | "doc" | "odt" => (set.word, CODE),                   // blue
        "xlsx" | "xls" | "ods" | "csv" | "tsv" => (set.excel, SHEET), // green
        "pptx" | "ppt" | "odp" => (set.ppt, MEDIA),                   // peach
        "ipynb" => (set.default, DOC),                                // notepad

        // Archives & Disks
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" => {
            (set.archive, ARCHIVE)
        }
        "iso" | "img" => (set.default, ARCHIVE),

        // Media
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tiff" | "ico" | "svg" => {
            (set.image, IMAGE)
        }
        "mp4" | "m4v" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "mpg" | "mpeg" | "mts"
        | "m2ts" | "3gp" | "vob" | "ogv" => (set.video, MEDIA),
        "mp3" | "flac" | "wav" | "ogg" | "aac" | "m4a" | "opus" => (set.audio, MEDIA),

        // Binaries / Executables
        "exe" | "msi" | "apk" | "ipa" => (set.bin, BIN),
        "dll" | "so" | "dylib" => (set.bin, BIN),
        "ttf" | "otf" | "woff" | "woff2" => (set.default, PLAIN),

        _ => (set.default, PLAIN),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Status bar — also surfaces non-blocking notices
// ─────────────────────────────────────────────────────────────────────────────

fn draw_status_bar(f: &mut Frame, app: &AppState, area: Rect) {
    let t = app.theme();
    let C_TEXT = t.text;
    let C_MUTED = t.muted;
    let C_WARN = t.warn;
    let C_OK = t.ok;
    let C_ERR = t.err;

    if let Some(operation) = &app.operation_status {
        let percent = operation
            .done
            .saturating_mul(100)
            .checked_div(operation.total)
            .unwrap_or(100);
        let message = format!(
            "  {}  {}/{}  {}%  ·  Esc cancel",
            operation.label, operation.done, operation.total, percent,
        );
        f.render_widget(
            Paragraph::new(truncate(&message, area.width as usize)).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(C_WARN)
                    .add_modifier(Modifier::BOLD),
            ),
            area,
        );
        return;
    }

    if let Some((msg, is_err)) = app.active_notice() {
        let style = if is_err {
            Style::default()
                .fg(Color::White)
                .bg(C_ERR)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Black)
                .bg(C_OK)
                .add_modifier(Modifier::BOLD)
        };
        let icon = if is_err { "[X]" } else { "[OK]" };
        let message = truncate(&format!("  {} {}", icon, msg), area.width as usize);
        f.render_widget(Paragraph::new(message).style(style), area);
        return;
    }

    if let Some(path) = &app.navigation_loading {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| path.to_str().unwrap_or("folder"));
        let message = truncate(&format!("  Opening {name}…"), area.width as usize);
        f.render_widget(
            Paragraph::new(message).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(t.accent2)
                    .add_modifier(Modifier::BOLD),
            ),
            area,
        );
        return;
    }

    let cur = app.current();
    let count = cur.files.len();
    let pos = if count > 0 { cur.selected + 1 } else { 0 };
    let marked = cur.marked.len();
    let sel_info = if marked > 0 {
        format!(" │ {} marked", marked)
    } else {
        String::new()
    };

    let bg = t.bg_root;
    if app.search_active {
        let (position, count) = app.search_match_position();
        let text = if app.search_query.is_empty() {
            "  FIND  |  type a filename  |  Up/Down next match  |  Enter keep selection  |  Esc close".to_string()
        } else {
            format!(
                "  FIND \"{}\"  |  {} of {}  |  Up/Down next match  |  Enter keep selection  |  Esc close",
                app.search_query, position, count,
            )
        };
        f.render_widget(
            Paragraph::new(text).style(Style::default().fg(C_TEXT).bg(t.sel_bg_inactive)),
            area,
        );
        return;
    }

    if app.edit_preview_mode {
        let dirty = if app.edit_dirty { "UNSAVED" } else { "saved" };
        let text = format!(
            "  EDIT ({})  │  type naturally  │  arrows move cursor  │  Ctrl+S save  │  Esc leave editor",
            dirty
        );
        f.render_widget(
            Paragraph::new(text).style(Style::default().fg(C_TEXT).bg(bg)),
            area,
        );
        return;
    }

    if app.mode != AppMode::Normal {
        let text = match app.mode {
            AppMode::Rename => format!(
                " {}/{} │ RENAME: type new name · Enter confirm · Esc cancel",
                pos,
                count.max(1)
            ),
            AppMode::ConfirmDelete => {
                format!(" {}/{} │ DELETE: Y confirm · Esc cancel", pos, count.max(1))
            }
            AppMode::ConfirmDeletePermanent => format!(
                " {}/{} │ PERMANENTLY DELETE: Y confirm · Esc cancel",
                pos,
                count.max(1)
            ),
            AppMode::NewFolder => format!(
                " {}/{} │ NEW FOLDER: type name · Enter confirm · Esc cancel",
                pos,
                count.max(1)
            ),
            AppMode::NewFile => format!(
                " {}/{} │ NEW FILE: type name · Enter confirm · Esc cancel",
                pos,
                count.max(1)
            ),
            AppMode::ContextMenu => format!(
                " {}/{} │ MENU: click an action · Esc close",
                pos,
                count.max(1)
            ),
            AppMode::Properties => format!(" {}/{} │ PROPERTIES: Esc close", pos, count.max(1)),
            _ => String::new(),
        };
        let style = Style::default().fg(Color::Black).bg(C_WARN);
        f.render_widget(Paragraph::new(format!("  {}", text)).style(style), area);
        return;
    }

    let mut spans = vec![Span::styled(
        format!("  {}/{}{}  │  ", pos, count.max(1), sel_info),
        Style::default().fg(C_MUTED).bg(bg),
    )];

    let key_style = Style::default()
        .fg(C_TEXT)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
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

    spans.push(Span::styled("Ctrl+F", key_style));
    spans.push(Span::styled(" find    ", label_style));

    spans.push(Span::styled("q", key_style));
    spans.push(Span::styled(" quit", label_style));

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        page_control_rects, tile_grid_dimensions, truncate, PAGE_NEXT_LABEL, PAGE_PREV_LABEL,
        PILL_LEFT_CAP, PILL_RIGHT_CAP,
    };
    use ratatui::layout::Rect;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn pill_caps_use_the_original_full_height_geometry() {
        assert_eq!(format!("{PILL_LEFT_CAP}value{PILL_RIGHT_CAP}"), "value");
    }

    #[test]
    fn truncation_uses_terminal_cells_and_preserves_graphemes() {
        let value = truncate("தமிழ்-preview", 7);
        assert!(UnicodeWidthStr::width(value.as_str()) <= 7);
        assert!(!value.contains(char::REPLACEMENT_CHARACTER));
        assert!(value.ends_with('…'));
    }

    #[test]
    fn windows_tiles_reflow_without_losing_capacity() {
        assert_eq!(tile_grid_dimensions(29, 4), (1, 1, 1, 29));
        assert_eq!(tile_grid_dimensions(60, 10), (2, 2, 4, 30));
        assert_eq!(tile_grid_dimensions(95, 20), (3, 4, 12, 31));
    }

    #[test]
    fn page_controls_are_distinct_click_targets_on_the_controls_row() {
        let inner = Rect::new(40, 8, 80, 30);
        let (previous, next) = page_control_rects(inner);
        assert_eq!(previous.y, inner.y + 2);
        assert_eq!(next.y, inner.y + 2);
        assert_eq!(previous.width, PAGE_PREV_LABEL.len() as u16);
        assert_eq!(next.width, PAGE_NEXT_LABEL.len() as u16);
        assert!(previous.x + previous.width < next.x);
    }
}
