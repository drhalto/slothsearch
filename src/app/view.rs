//! Drawing and input for the launcher window.

use super::*;

impl eframe::App for SlothSearch {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg, ctx);
        }
        if !self.visible {
            self.web.trim();
        }
        if self.reload_pending {
            self.reload_pending = false;
            self.refresh_sources(ctx, true);
            self.check_health();
        }
        if self.visible {
            self.sync_query();
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.visible {
            return;
        }
        let empty = self.query.is_empty();
        let droppable = self.scope.is_some() || self.mode == Mode::Windows || self.filter.is_some();
        let mut taken = Vec::new();
        raw.events.retain(|e| {
            let Event::Key { key, pressed: true, modifiers: m, .. } = e else { return true };
            let plain = !m.ctrl && !m.alt && !m.shift;
            let cmd = match key {
                Key::ArrowDown if plain => KeyCmd::Down,
                Key::ArrowUp if plain => KeyCmd::Up,
                Key::Tab if m.ctrl => KeyCmd::CycleFilter(if m.shift { -1 } else { 1 }),
                Key::Tab if !m.ctrl && !m.alt => if m.shift { KeyCmd::Up } else { KeyCmd::Down },
                Key::N if m.ctrl && !m.shift => KeyCmd::Down,
                Key::P if m.ctrl && !m.shift => KeyCmd::Up,
                Key::PageDown => KeyCmd::PageDown,
                Key::PageUp => KeyCmd::PageUp,
                Key::Escape => KeyCmd::Escape,
                Key::K if m.ctrl => KeyCmd::ToggleActions,
                Key::Backspace if plain && empty && droppable => KeyCmd::DropScope,
                k if m.ctrl && !m.shift => match k {
                    Key::Num1 => KeyCmd::Nth(0),
                    Key::Num2 => KeyCmd::Nth(1),
                    Key::Num3 => KeyCmd::Nth(2),
                    Key::Num4 => KeyCmd::Nth(3),
                    Key::Num5 => KeyCmd::Nth(4),
                    Key::Num6 => KeyCmd::Nth(5),
                    Key::Num7 => KeyCmd::Nth(6),
                    Key::Num8 => KeyCmd::Nth(7),
                    Key::Num9 => KeyCmd::Nth(8),
                    _ => match combo(*key, *m) {
                        Some(c) => KeyCmd::Combo(c),
                        None => return true,
                    },
                },
                _ => match combo(*key, *m) {
                    Some(c) => KeyCmd::Combo(c),
                    None => return true,
                },
            };
            taken.push(cmd);
            false
        });
        self.keys.extend(taken);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // eframe shows the window after its first frame; it starts off-screen, then hides.
        if self.boot_frames < 3 {
            self.boot_frames += 1;
            if self.boot_frames == 3 && !self.visible {
                make_popup(self.hwnd);
                shell::hide(self.hwnd);
                if self.demo.is_some() {
                    shell::post_show();
                }
            }
            ctx.request_repaint();
            return;
        }
        // Hiding takes two frames: an empty one (so the next show doesn't flash stale
        // results), then the actual hide.
        match self.hiding {
            2 => {
                self.hiding = 1;
                ctx.request_repaint();
                return;
            }
            1 => {
                self.hiding = 0;
                self.visible = false;
                self.hidden_at = Instant::now();
                self.keys.clear();
                shell::hide(self.hwnd);
                return;
            }
            _ => {}
        }
        if !self.visible {
            return;
        }

        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(true) {
            self.had_focus = true;
        } else if focused == Some(false) && self.had_focus && self.shown_at.elapsed() > Duration::from_millis(250) {
            if shell::is_foreground(self.hwnd) {
                // Focus went into the web preview (a click or scroll there), not away.
                // Once the mouse leaves the page, typing goes back to the search box.
                let over_web = ctx.input(|i| i.pointer.hover_pos()).zip(self.web_rect).is_some_and(|(p, r)| r.contains(p));
                if !over_web {
                    self.web.focus_parent();
                }
            } else {
                self.begin_hide();
                ctx.request_repaint();
                return;
            }
        }

        self.sync_query();
        self.handle_keys(&ctx);
        if self.hiding > 0 {
            ctx.request_repaint();
            return;
        }

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        // A pointer that was already resting where the launcher opened hasn't moved: only
        // real movement selects the row under it.
        let moved = self.pointer_seen && pointer.is_some() && pointer != self.last_pointer;
        self.pointer_seen |= pointer.is_some();
        self.last_pointer = pointer;

        let t = ctx.animate_bool_with_time(egui::Id::new("show"), true, 0.09);
        ui.multiply_opacity(t);

        self.web_target = None;
        let has_list = !self.sections.is_empty() || !self.query.trim().is_empty();
        let target_h = if has_list { SEARCH_H + LIST_H + FOOTER_H } else { SEARCH_H };
        let panel_h = ctx.animate_value_with_time(egui::Id::new("panel-h"), target_h, 0.12);
        // The web preview waits for the panel to finish opening: loading into a viewport
        // that's still growing leaves the page scrolled.
        self.panel_settled = (panel_h - target_h).abs() < 0.5 && t >= 1.0;
        let top = MARGIN + (1.0 - t) * 6.0;
        let panel = Rect::from_min_size(pos2(MARGIN, top), vec2(PANEL_W, panel_h));

        // A click outside the panel (but inside our transparent window) dismisses it.
        if ctx.input(|i| i.pointer.any_pressed()) && pointer.is_some_and(|p| !panel.contains(p)) {
            self.begin_hide();
            ctx.request_repaint();
            return;
        }

        let p = ui.painter().clone();
        let pal = self.pal;
        let r = CornerRadius::same(RADIUS);
        p.add(Shadow { offset: [0, 12], blur: 36, spread: 0, color: pal.shadow }.as_shape(panel, r));
        p.rect_filled(panel, r, pal.panel);
        p.rect_stroke(panel, r, Stroke::new(1.0, pal.border), StrokeKind::Inside);

        let search_rect = Rect::from_min_size(panel.min, vec2(PANEL_W, SEARCH_H));
        self.search_bar(ui, search_rect);

        if has_list && panel_h > SEARCH_H + 20.0 {
            let body_top = panel.top() + SEARCH_H;
            let body_h = (panel_h - SEARCH_H - FOOTER_H).max(0.0);
            p.hline(panel.x_range().shrink(1.0), body_top, Stroke::new(1.0, pal.divider));
            let list_rect = Rect::from_min_size(pos2(panel.left(), body_top), vec2(LIST_W, body_h));
            let preview_rect = Rect::from_min_max(pos2(panel.left() + LIST_W, body_top), pos2(panel.right(), body_top + body_h));
            let footer_rect = Rect::from_min_max(pos2(panel.left(), panel.bottom() - FOOTER_H), panel.max);

            ui.scope_builder(UiBuilder::new().max_rect(list_rect), |ui| {
                ui.set_clip_rect(list_rect);
                self.list(ui, list_rect, moved);
            });
            if body_h > 60.0 {
                p.vline(preview_rect.left(), preview_rect.y_range().shrink(10.0), Stroke::new(1.0, pal.divider));
                ui.scope_builder(UiBuilder::new().max_rect(preview_rect), |ui| {
                    ui.set_clip_rect(preview_rect);
                    self.preview(ui, preview_rect.shrink2(vec2(22.0, 18.0)));
                });
            }
            p.hline(panel.x_range().shrink(1.0), footer_rect.top(), Stroke::new(1.0, pal.divider));
            self.footer(ui, footer_rect);
            if self.actions_open {
                self.actions_menu(ui, panel, footer_rect.top(), moved);
            }
            if self.filter_menu {
                self.filter_menu_ui(ui, panel, moved);
            }
        }

        self.apply_web(&ctx);

        // Keep animating toasts, the spinner and confirm prompts.
        if self.pending_since.is_some() || self.toast.is_some() || self.armed.is_some() || t < 1.0 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

impl SlothSearch {
    fn handle_keys(&mut self, ctx: &egui::Context) {
        let keys = std::mem::take(&mut self.keys);
        for k in keys {
            if self.filter_menu {
                let n = filters::all().len() + 1;
                match k {
                    KeyCmd::Down => self.filter_sel = (self.filter_sel + 1).min(n - 1),
                    KeyCmd::Up => self.filter_sel = self.filter_sel.saturating_sub(1),
                    KeyCmd::Escape => self.filter_menu = false,
                    KeyCmd::Combo(c) if c == "Enter" => self.pick_filter(self.filter_sel),
                    KeyCmd::CycleFilter(d) => self.cycle_filter(d),
                    _ => {}
                }
                continue;
            }
            if self.renaming.is_some() {
                match k {
                    KeyCmd::Escape => {
                        self.renaming = None;
                        self.query.clear();
                        self.built = None;
                    }
                    KeyCmd::Combo(c) if c == "Enter" => {
                        if let Some(item) = self.renaming.take() {
                            self.engine.favorites.rename(&item, &self.query);
                        }
                        self.query.clear();
                        self.built = None;
                        self.toast = Some(("Renamed".into(), Instant::now(), false));
                    }
                    _ => {}
                }
                continue;
            }
            if self.actions_open {
                let n = self.selected_item().cloned().map_or(0, |it| self.actions_for(&it).len());
                match k {
                    KeyCmd::Down => self.action_sel = (self.action_sel + 1).min(n.saturating_sub(1)),
                    KeyCmd::Up => self.action_sel = self.action_sel.saturating_sub(1),
                    KeyCmd::Escape | KeyCmd::ToggleActions => self.actions_open = false,
                    KeyCmd::Combo(c) if c == "Enter" => {
                        let acts = self.selected_item().cloned().map(|it| self.actions_for(&it)).unwrap_or_default();
                        if let Some(a) = acts.get(self.action_sel) {
                            let act = a.act;
                            self.actions_open = false;
                            self.run(act, self.selected);
                        }
                    }
                    KeyCmd::Combo(c) => self.combo(&c),
                    _ => {}
                }
                continue;
            }
            let n = self.flat.len();
            match k {
                KeyCmd::Down if n > 0 => self.select((self.selected + 1).min(n - 1)),
                KeyCmd::Up => self.select(self.selected.saturating_sub(1)),
                KeyCmd::PageDown if n > 0 => self.select((self.selected + 8).min(n - 1)),
                KeyCmd::PageUp => self.select(self.selected.saturating_sub(8)),
                KeyCmd::Escape => {
                    if self.query.is_empty() {
                        self.begin_hide();
                    } else {
                        self.query.clear();
                        self.focus_input = true;
                    }
                }
                KeyCmd::ToggleActions => {
                    if self.selected_item().is_some() {
                        self.actions_open = true;
                        self.action_sel = 0;
                    }
                }
                KeyCmd::DropScope => {
                    // Peel off one thing at a time: the type filter, then the folder.
                    if self.filter.is_some() {
                        self.filter = None;
                    } else {
                        self.scope = None;
                        self.mode = Mode::All;
                    }
                }
                KeyCmd::CycleFilter(d) => self.cycle_filter(d),
                KeyCmd::Nth(i) => {
                    if i < n {
                        self.selected = i;
                        self.run(Act::Open, i);
                    }
                }
                KeyCmd::Combo(c) if c == "Ctrl+," => {
                    open_settings();
                    self.begin_hide();
                }
                KeyCmd::Combo(c) => self.combo(&c),
                _ => {}
            }
        }
        if self.hiding > 0 {
            ctx.request_repaint();
        }
    }

    fn actions_for(&self, item: &Item) -> Vec<launch::Action> {
        let mut v = launch::actions(item, self.scope.as_ref());
        if !self.engine.history.has(item) {
            v.retain(|a| a.act != Act::Forget);
        }
        if item.key().is_some() {
            let fav = self.engine.favorites.contains(item);
            let (label, g) = if fav { ("Remove from Favorites", glyph::STAR_FILLED) } else { ("Add to Favorites", glyph::STAR) };
            v.insert(v.len().min(1), launch::Action { act: Act::Favorite, label: label.into(), keys: "Ctrl+D", glyph: g });
            // Arranging only makes sense where favorites are listed: the empty search box.
            let home = self.query.trim().is_empty() && self.scope.is_none() && self.filter.is_none() && self.mode == Mode::All;
            if fav && home {
                v.push(launch::Action { act: Act::MoveUp, label: "Move up".into(), keys: "Ctrl+Shift+Up", glyph: glyph::UP });
                v.push(launch::Action { act: Act::MoveDown, label: "Move down".into(), keys: "Ctrl+Shift+Down", glyph: glyph::DOWN });
                v.push(launch::Action { act: Act::Rename, label: "Rename".into(), keys: "F2", glyph: glyph::RENAME });
            }
        }
        v
    }

    /// Ctrl+Tab: everything -> apps -> files -> folders -> PDFs -> ... -> everything.
    fn cycle_filter(&mut self, d: i32) {
        let n = filters::CYCLE.len() as i32 + 1;
        let cur = self.effective().0.and_then(|f| filters::CYCLE.iter().position(|k| *k == f.key)).map_or(0, |i| i as i32 + 1);
        let next = (cur + d).rem_euclid(n);
        self.filter = if next == 0 { None } else { filters::lookup(filters::CYCLE[next as usize - 1]) };
        if self.query.to_lowercase().contains("type:") {
            self.query = filters::parse(&self.query).1;
            self.cursor_end = true;
        }
    }

    /// From the filter menu: 0 is "All types", then `filters::all()` in order.
    fn pick_filter(&mut self, i: usize) {
        self.filter = if i == 0 { None } else { filters::all().get(i - 1).and_then(|(k, _, _)| filters::lookup(k)) };
        if self.query.to_lowercase().contains("type:") {
            self.query = filters::parse(&self.query).1;
            self.cursor_end = true;
        }
        self.filter_menu = false;
        self.focus_input = true;
    }

    /// Runs the selected row's action bound to this shortcut, if it has one.
    fn combo(&mut self, keys: &str) {
        let Some(item) = self.selected_item().cloned() else { return };
        let acts = self.actions_for(&item);
        let act = acts.iter().find(|a| a.keys == keys).map(|a| a.act).or_else(|| {
            // Shift+Enter is just Enter.
            (keys == "Shift+Enter").then(|| acts.first().map(|a| a.act)).flatten()
        });
        if let Some(act) = act {
            self.actions_open = false;
            self.run(act, self.selected);
        }
    }

    fn select(&mut self, i: usize) {
        if i != self.selected {
            self.selected = i;
            self.armed = None;
        }
        self.user_moved = true;
        self.scroll_to_selected = true;
    }

    // -----------------------------------------------------------------------------------

    fn search_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let pal = self.pal;
        let p = ui.painter();
        let cy = rect.center().y;
        p.text(pos2(rect.left() + 24.0, cy), Align2::LEFT_CENTER, glyph::SEARCH.to_string(), icon_font(17.0), pal.dim);
        let mut x = rect.left() + 54.0;

        // Scope chip: "roost" when opened over that Explorer folder, "Windows" in switcher mode.
        let chip = match (&self.scope, self.mode) {
            (_, Mode::Windows) => Some((glyph::WINDOW, "Windows".to_string())),
            (Some(s), _) => Some((glyph::FOLDER, crate::util::file_name(&s.path).to_string())),
            _ => None,
        };
        let draw_chip = |x: f32, g: char, label: String, fill: Color32, fg: Color32| -> f32 {
            let label: String = if label.chars().count() > 22 { label.chars().take(21).chain(['…']).collect() } else { label };
            let galley = p.layout_no_wrap(label, semibold(13.0), fg);
            let w = galley.size().x + 38.0;
            let chip_rect = Rect::from_center_size(pos2(x + w / 2.0, cy), vec2(w, 28.0));
            p.rect_filled(chip_rect, 8.0, fill);
            p.text(pos2(chip_rect.left() + 11.0, cy), Align2::LEFT_CENTER, g.to_string(), icon_font(12.5), fg);
            p.galley(pos2(chip_rect.left() + 29.0, cy - galley.size().y / 2.0), galley, fg);
            chip_rect.right() + 8.0
        };
        if let Some((g, label)) = chip {
            x = draw_chip(x, g, label, pal.accent.gamma_multiply(0.22), pal.accent_text);
        }
        // The type filter (or the rename in progress) gets a neutral chip of its own.
        let (filter, _) = self.effective();
        let second = match (&self.renaming, &filter) {
            (Some(_), _) => Some((glyph::RENAME, "Rename".to_string())),
            (None, Some(f)) => Some((f.glyph, f.label.clone())),
            _ => None,
        };
        if let Some((g, label)) = second {
            x = draw_chip(x, g, label, pal.kbd, pal.text);
        }
        if x > rect.left() + 54.0 {
            x += 2.0;
        }

        // The filter menu button, at the right end of the bar.
        let button_label = filter.as_ref().map_or("All types".to_string(), |f| f.label.clone());
        let bg = p.layout_no_wrap(button_label, regular(12.5), pal.dim);
        let bw = bg.size().x + 52.0;
        let button = Rect::from_min_size(pos2(rect.right() - 16.0 - bw, cy - 14.0), vec2(bw, 28.0));
        self.filter_button = button;
        let bresp = ui.interact(button, egui::Id::new("filter-button"), Sense::click());
        let p = ui.painter();
        let fill = if self.filter_menu || bresp.hovered() { pal.selected } else { pal.tile };
        p.rect_filled(button, 8.0, fill);
        p.text(pos2(button.left() + 11.0, cy), Align2::LEFT_CENTER, glyph::FILTER.to_string(), icon_font(11.5), pal.dim);
        p.galley(pos2(button.left() + 28.0, cy - bg.size().y / 2.0), bg, pal.dim);
        p.text(pos2(button.right() - 10.0, cy), Align2::RIGHT_CENTER, '\u{E70D}'.to_string(), icon_font(8.5), pal.dim);
        bresp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Filter by type"));
        if bresp.clicked() {
            self.filter_menu = !self.filter_menu;
            self.actions_open = false;
            self.filter_sel = filter.as_ref().and_then(|f| filters::all().iter().position(|(k, _, _)| *k == f.key)).map_or(0, |i| i + 1);
        }

        let hint = match (&self.renaming, &self.scope, self.mode) {
            (Some(_), _, _) => "New name",
            (_, _, Mode::Windows) => "Switch to a window",
            (_, Some(_), _) => "Search this folder",
            _ if filter.is_some() => "Search, or leave empty for the newest",
            _ => "Search apps, files, windows and more",
        };
        let spinner_w = 30.0;
        let edit_rect = Rect::from_min_max(pos2(x, rect.top() + 8.0), pos2(button.left() - spinner_w, rect.bottom() - 8.0));
        let id = egui::Id::new("query");
        let out = ui
            .scope_builder(UiBuilder::new().max_rect(edit_rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                TextEdit::singleline(&mut self.query)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .font(regular(20.0))
                    .text_color(pal.text)
                    .hint_text(RichText::new(hint).color(pal.faint).font(regular(20.0)))
                    .desired_width(edit_rect.width())
                    .show(ui)
            })
            .inner;
        if self.focus_input {
            out.response.request_focus();
            self.focus_input = false;
        }
        if self.cursor_end {
            self.cursor_end = false;
            let mut state = out.state.clone();
            let n = self.query.chars().count();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(n))));
            state.store(ui.ctx(), id);
        }
        if self.select_all {
            self.select_all = false;
            let mut state = out.state;
            let n = self.query.chars().count();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
            state.store(ui.ctx(), id);
        }
        if !out.response.has_focus() && self.visible && !self.actions_open && !self.filter_menu {
            out.response.request_focus();
        }

        // A quiet spinner while Everything is answering (only if it's slow enough to see).
        if self.pending_since.is_some_and(|t| t.elapsed() > Duration::from_millis(120)) {
            let c = pos2(button.left() - 16.0, cy);
            let t = ui.input(|i| i.time) as f32;
            let pts: Vec<Pos2> = (0..=16)
                .map(|k| {
                    let a = t * 6.0 + k as f32 / 16.0 * std::f32::consts::PI * 1.3;
                    c + vec2(a.cos(), a.sin()) * 7.0
                })
                .collect();
            ui.painter().add(egui::Shape::line(pts, Stroke::new(1.6, pal.faint)));
        }
    }

    fn list(&mut self, ui: &mut Ui, rect: Rect, moved: bool) {
        let pal = self.pal;
        let sections = std::mem::take(&mut self.sections);
        let mut clicked = None;
        let mut context = None;
        let mut hovered = None;
        let scroll = std::mem::take(&mut self.scroll_to_selected);

        // Nothing but "Search the web" means nothing on this PC matched.
        let searching = !self.query.trim().is_empty() || self.filter.is_some();
        let nothing = searching && sections.iter().all(|s| s.items.iter().all(|i| i.kind == Kind::Web));
        if nothing {
            let p = ui.painter();
            if self.pending_since.is_some() {
                p.text(rect.center() + vec2(0.0, 20.0), Align2::CENTER_CENTER, "Searching…", regular(14.0), pal.faint);
            } else {
                // The mascot owns up to it.
                let art = Rect::from_center_size(rect.center() + vec2(0.0, 6.0), vec2(132.0, 132.0));
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                p.image(self.sloth_mascot.id(), art, uv, Color32::WHITE);
                p.text(pos2(rect.center().x, art.bottom() + 20.0), Align2::CENTER_CENTER, "No results", semibold(15.0), pal.text);
                p.text(
                    pos2(rect.center().x, art.bottom() + 42.0),
                    Align2::CENTER_CENTER,
                    "I looked everywhere. Quickly, for once.",
                    regular(12.5),
                    pal.dim,
                );
            }
        }

        ScrollArea::vertical()
            .id_salt("results")
            .auto_shrink([false, false])
            .max_height(rect.height())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(6.0);
                let mut flat = 0usize;
                for sec in &sections {
                    let (hr, _) = ui.allocate_exact_size(vec2(rect.width(), HEADER_H), Sense::hover());
                    ui.painter().text(
                        pos2(hr.left() + 22.0, hr.bottom() - 9.0),
                        Align2::LEFT_BOTTOM,
                        &sec.title,
                        semibold(12.0),
                        pal.faint,
                    );
                    for item in &sec.items {
                        let selected = flat == self.selected;
                        let resp = self.row(ui, item, selected, flat, rect.width());
                        if selected && scroll {
                            resp.scroll_to_me(None);
                        }
                        if resp.hovered() && moved {
                            hovered = Some(flat);
                        }
                        if resp.clicked() {
                            clicked = Some(flat);
                        }
                        if resp.secondary_clicked() {
                            context = Some(flat);
                        }
                        flat += 1;
                    }
                }
                ui.add_space(8.0);
            });
        self.sections = sections;

        if let Some(i) = hovered {
            if !self.actions_open && i != self.selected {
                self.selected = i;
                self.user_moved = true;
                self.armed = None;
            }
        }
        if let Some(i) = context {
            self.selected = i;
            self.user_moved = true;
            self.actions_open = true;
            self.action_sel = 0;
        } else if let Some(i) = clicked {
            self.selected = i;
            self.actions_open = false;
            self.run(Act::Open, i);
        }
    }

    fn row(&mut self, ui: &mut Ui, item: &Item, selected: bool, index: usize, width: f32) -> egui::Response {
        let pal = self.pal;
        let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
        resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, format!("{}, {}", item.title, item.kind.label())));
        let p = ui.painter();
        let bg = rect.shrink2(vec2(8.0, 2.0));
        if selected {
            p.rect_filled(bg, 8.0, pal.selected);
        } else if resp.hovered() {
            p.rect_filled(bg, 8.0, pal.hover);
        }

        let icon_rect = Rect::from_center_size(pos2(rect.left() + 34.0, rect.center().y), vec2(28.0, 28.0));
        self.paint_icon(ui, &item.icon, icon_rect, item.kind);

        // Right side: when it was modified for files, the kind otherwise; Ctrl+n hints.
        let right = match item.kind {
            Kind::File | Kind::Folder => item.modified.map(human_age).unwrap_or_default(),
            Kind::Command if matches!(&item.target, Target::Command { id } if self.armed.as_ref().is_some_and(|(a, _)| a == id)) => {
                "Press Enter again".into()
            }
            Kind::Command => item.subtitle.clone(),
            k => k.label().to_string(),
        };
        let armed = right == "Press Enter again";
        let p = ui.painter();
        let right_galley = p.layout_no_wrap(right, regular(12.0), if armed { pal.warn } else { pal.faint });
        let right_x = rect.right() - 22.0;
        p.galley(pos2(right_x - right_galley.size().x, rect.center().y - right_galley.size().y / 2.0), right_galley.clone(), pal.faint);

        let text_left = icon_rect.right() + 12.0;
        let max_w = (right_x - right_galley.size().x - 14.0 - text_left).max(40.0);
        let show_sub = !item.subtitle.is_empty() && item.kind != Kind::Command;
        let title = highlighted(&item.title, &item.hl, pal, max_w, item.kind == Kind::Calc);
        let title_h = title.size().y;
        if show_sub {
            let sub = truncated(&item.subtitle, regular(12.0), pal.dim, max_w);
            let total = title_h + sub.size().y - 1.0;
            let y = rect.center().y - total / 2.0;
            p.galley(pos2(text_left, y), title, pal.text);
            p.galley(pos2(text_left, y + title_h - 1.0), sub, pal.dim);
        } else {
            p.galley(pos2(text_left, rect.center().y - title_h / 2.0), title, pal.text);
        }
        if index < 9 && ui.input(|i| i.modifiers.ctrl) {
            let k = Rect::from_center_size(pos2(rect.left() + 12.0, rect.center().y), vec2(14.0, 16.0));
            p.rect_filled(k, 4.0, pal.kbd);
            p.text(k.center(), Align2::CENTER_CENTER, (index + 1).to_string(), regular(10.5), pal.dim);
        }
        resp
    }

    fn texture(&mut self, key: &IconKey, px: u32) -> Option<egui::TextureId> {
        let slot = (key.clone(), px);
        match self.textures.get(&slot) {
            Some(IconState::Ready(t)) => Some(t.id()),
            Some(_) => None,
            None => {
                self.textures.insert(slot, IconState::Pending);
                self.icons.request(key.clone(), px);
                None
            }
        }
    }

    fn paint_icon(&mut self, ui: &Ui, icon: &Icon, rect: Rect, kind: Kind) {
        let pal = self.pal;
        // Shell icons come in 16/32/48/256; ask for one at least as big as the slot.
        let need = rect.width() * ui.ctx().pixels_per_point();
        let px = if need <= 48.0 { 48 } else { 256 };
        match icon {
            Icon::Shell(key) => {
                if let Some(tex) = self.texture(key, px) {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    ui.painter().image(tex, rect, uv, Color32::WHITE);
                } else if matches!(self.textures.get(&(key.clone(), px)), Some(IconState::Failed)) {
                    let g = match kind {
                        Kind::Folder | Kind::Project => glyph::FOLDER,
                        Kind::File => glyph::DOCUMENT,
                        Kind::Window => glyph::WINDOW,
                        _ => glyph::APP,
                    };
                    glyph_tile(ui, g, rect, pal, false);
                }
            }
            Icon::Glyph(g) => glyph_tile(ui, *g, rect, pal, kind == Kind::Calc),
        }
    }

    // -----------------------------------------------------------------------------------

    /// What the web preview should show for a row, if anything.
    fn web_source(item: &Item) -> Option<Source> {
        let local = |p: &str| crate::util::is_local_drive(p);
        match &item.target {
            Target::Path { path, folder: false } if local(path) => Source::for_path(path),
            Target::Project { path } => {
                let readme = ["README.md", "readme.md", "Readme.md"].iter().map(|n| format!("{path}\\{n}")).find(|p| std::path::Path::new(p).exists())?;
                Some(Source::Markdown(readme))
            }
            Target::Url { url } if url.starts_with("http://") || url.starts_with("https://") => Some(Source::Url(url.clone())),
            _ => None,
        }
    }

    fn preview(&mut self, ui: &mut Ui, rect: Rect) {
        let pal = self.pal;
        let Some(item) = self.selected_item().cloned() else { return };
        let p = ui.painter().clone();
        let mut y = rect.top() + 4.0;
        let cx = rect.center().x;

        // Pages, Markdown, SVG and PDF get the live web preview, with a caption below.
        if let Some(src) = Self::web_source(&item).filter(|_| !self.web.broken) {
            if !self.panel_settled {
                ui.ctx().request_repaint_after(Duration::from_millis(30));
            }
            let web_rect = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 58.0));
            p.rect_filled(web_rect, 8.0, pal.code_bg);
            p.text(web_rect.center(), Align2::CENTER_CENTER, "Loading preview…", regular(12.5), pal.faint);
            if self.panel_settled {
                self.web_target = Some((src.clone(), web_rect));
            }
            let title = match &src {
                Source::Markdown(r) if matches!(item.target, Target::Project { .. }) => format!("{} · {}", item.title, crate::util::file_name(r)),
                _ => item.title.clone(),
            };
            let g = truncated(&title, semibold(13.5), pal.text, rect.width());
            p.galley(pos2(rect.left(), web_rect.bottom() + 12.0), g, pal.text);
            let sub = match &item.target {
                Target::Project { path } => {
                    let b = projects::branch(path).unwrap_or_default();
                    let r = projects::web_url(path).map(|u| u.trim_start_matches("https://").to_string()).unwrap_or_default();
                    [b, r].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
                }
                Target::Url { url } => url.clone(),
                Target::Path { path, .. } => {
                    let mut parts = vec![tilde(crate::util::parent(path))];
                    if let Some(s) = item.size {
                        parts.push(human_size(s));
                    }
                    if let Some(m) = item.modified {
                        parts.push(human_age(m));
                    }
                    parts.join(" · ")
                }
                _ => String::new(),
            };
            let g = truncated(&sub, regular(12.0), pal.dim, rect.width());
            p.galley(pos2(rect.left(), web_rect.bottom() + 32.0), g, pal.dim);
            return;
        }

        // The picture: a calculator answer, a text excerpt, a thumbnail, or a big icon.
        let path = item.target.path().map(str::to_string);
        let ext = path.as_deref().map(extension).unwrap_or_default();
        let text_lines = match (&item.target, &path) {
            (Target::Path { folder: false, .. }, Some(pth)) if is_texty(&ext) => self.text_preview(pth, item.size),
            _ => None,
        };
        if let Target::Calc { .. } = item.target {
            p.text(pos2(cx, y + 30.0), Align2::CENTER_CENTER, &item.subtitle, regular(14.0), pal.dim);
            let g = truncated(&item.title, semibold(30.0), pal.text, rect.width());
            p.galley(pos2(cx - g.size().x / 2.0, y + 52.0), g, pal.text);
            p.text(pos2(cx, y + 116.0), Align2::CENTER_CENTER, "Enter copies the answer", regular(12.0), pal.faint);
            return;
        } else if let Some(lines) = text_lines {
            let box_rect = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), 176.0));
            p.rect_filled(box_rect, 8.0, pal.code_bg);
            let mut ty = box_rect.top() + 10.0;
            for line in lines.iter().take(11) {
                let g = truncated(line, mono(11.5), pal.dim, box_rect.width() - 20.0);
                let h = g.size().y;
                p.galley(pos2(box_rect.left() + 10.0, ty), g, pal.dim);
                ty += h + 0.5;
            }
            y = box_rect.bottom() + 16.0;
        } else if matches!(item.target, Target::Path { folder: false, .. }) && is_thumbable(&ext) {
            let key = IconKey::Thumb(path.clone().unwrap_or_default());
            let box_rect = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), 176.0));
            match self.texture(&key, 256) {
                Some(tex) => {
                    let size = ui.ctx().tex_manager().read().meta(tex).map(|m| m.size).unwrap_or([1, 1]);
                    let (w, h) = (size[0] as f32, size[1] as f32);
                    let s = (box_rect.width() / w).min(box_rect.height() / h).min(2.0);
                    let r = Rect::from_center_size(box_rect.center(), vec2(w * s, h * s));
                    p.image(tex, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    let r = Rect::from_center_size(box_rect.center(), vec2(64.0, 64.0));
                    self.paint_icon(ui, &item.icon, r, item.kind);
                }
            }
            y = box_rect.bottom() + 16.0;
        } else {
            let r = Rect::from_center_size(pos2(cx, y + 40.0), vec2(64.0, 64.0));
            self.paint_icon(ui, &item.icon, r, item.kind);
            y += 96.0;
        }

        // Title and kind.
        let mut job = LayoutJob::simple(item.title.clone(), semibold(15.0), pal.text, rect.width());
        job.halign = egui::Align::Center;
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let g = p.layout_job(job);
        p.galley(pos2(cx, y), g.clone(), pal.text);
        y += g.size().y + 2.0;
        let kind = match &item.target {
            Target::Path { folder: false, .. } if !ext.is_empty() => format!("{} file", ext.to_uppercase()),
            Target::App { id, exe: None } if id.contains('!') => "Microsoft Store app".into(),
            _ => item.kind.label().to_string(),
        };
        p.text(pos2(cx, y + 8.0), Align2::CENTER_CENTER, kind, regular(12.0), pal.dim);
        y += 28.0;
        p.hline(rect.x_range(), y, Stroke::new(1.0, pal.divider));
        y += 12.0;

        // Details.
        let mut rows: Vec<(&str, String)> = Vec::new();
        match &item.target {
            Target::Path { path, folder } => {
                rows.push(("Where", tilde(crate::util::parent(path))));
                if *folder {
                    if let Some(n) = self.folder_count(path) {
                        rows.push(("Contains", if n >= 1000 { "1,000+ items".into() } else { format!("{n} items") }));
                    }
                } else if let Some(s) = item.size {
                    rows.push(("Size", human_size(s)));
                }
                if let Some(m) = item.modified {
                    let age = human_age(m);
                    let date = crate::util::civil_date(m);
                    rows.push(("Modified", if age == date { date } else { format!("{age} · {date}") }));
                }
            }
            Target::App { exe, id } => match exe {
                Some(e) => rows.push(("Program", tilde(e))),
                None => rows.push(("App ID", id.clone())),
            },
            Target::Project { path } => {
                rows.push(("Where", tilde(path)));
                if let Some(b) = projects::branch(path) {
                    rows.push(("Branch", b));
                }
                if let Some(u) = projects::web_url(path) {
                    rows.push(("Remote", u.trim_start_matches("https://").to_string()));
                }
            }
            Target::Window { exe, .. } => {
                rows.push(("App", item.subtitle.clone()));
                if !exe.is_empty() {
                    rows.push(("Program", tilde(exe)));
                }
            }
            Target::Command { id } => {
                if let Some(c) = commands::find(id) {
                    rows.push(("Type", item.subtitle.clone()));
                    if let commands::Run::Open(u) = c.run {
                        rows.push(("Opens", u.to_string()));
                    }
                    if c.confirm {
                        rows.push(("Note", "Asks you to press Enter twice".into()));
                    }
                }
            }
            Target::Url { url } => rows.push(("Opens", url.trim_start_matches("everything:").to_string())),
            Target::Web { .. } => rows.push(("Opens", "your default browser".into())),
            Target::Complete { query } => rows.push(("Search", query.trim().to_string())),
            Target::Calc { .. } => {}
        }
        for (label, value) in rows {
            if y > rect.bottom() - 16.0 {
                break;
            }
            p.text(pos2(rect.left(), y), Align2::LEFT_TOP, label, regular(12.0), pal.faint);
            let mut job = LayoutJob::simple(value, regular(12.5), pal.text, rect.width() - 76.0);
            job.wrap.max_rows = 3;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let g = p.layout_job(job);
            let h = g.size().y;
            p.galley(pos2(rect.left() + 76.0, y - 1.0), g, pal.text);
            y += h.max(16.0) + 7.0;
        }
    }

    fn text_preview(&mut self, path: &str, size: Option<u64>) -> Option<Vec<String>> {
        if let Some(v) = self.text_previews.get(path) {
            return v.clone();
        }
        // Local drives only; a network share could stall the UI.
        let local = crate::util::is_local_drive(path);
        let lines = (local && size.unwrap_or(0) < 50_000_000)
            .then(|| {
                use std::io::Read;
                let mut buf = vec![0u8; 4096];
                let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).ok()?;
                buf.truncate(n);
                if buf.contains(&0) {
                    return None; // binary
                }
                let text = String::from_utf8_lossy(&buf).replace('\t', "    ");
                let v: Vec<String> = text.lines().take(14).map(|l| l.chars().take(160).collect()).collect();
                (!v.is_empty()).then_some(v)
            })
            .flatten();
        if self.text_previews.len() > 300 {
            self.text_previews.clear();
        }
        self.text_previews.insert(path.to_string(), lines.clone());
        lines
    }

    fn folder_count(&mut self, path: &str) -> Option<usize> {
        if let Some(v) = self.folder_counts.get(path) {
            return *v;
        }
        let local = crate::util::is_local_drive(path);
        let n = local.then(|| std::fs::read_dir(path).ok().map(|d| d.take(1000).count())).flatten();
        self.folder_counts.insert(path.to_string(), n);
        n
    }

    // -----------------------------------------------------------------------------------

    fn footer(&mut self, ui: &mut Ui, rect: Rect) {
        let pal = self.pal;
        let p = ui.painter().clone();
        let cy = rect.center().y;
        let left = rect.left() + 20.0;

        if let Some((_, t, _)) = &self.toast {
            if t.elapsed() > Duration::from_millis(2200) {
                self.toast = None;
            }
        }
        if self.armed.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(4)) {
            self.armed = None;
        }

        // Left: the most useful thing to say right now.
        let (g, text, color) = if let Some((t, _, err)) = &self.toast {
            (if *err { glyph::WARNING } else { '\u{E73E}' }, t.clone(), if *err { pal.warn } else { pal.accent_text })
        } else if let Some((id, _)) = &self.armed {
            let name = commands::find(id).map_or("do that", |c| c.name);
            (glyph::WARNING, format!("Press Enter again to {}", name.to_lowercase()), pal.warn)
        } else if let Some(w) = &self.warning {
            (glyph::WARNING, w.clone(), pal.warn)
        } else if self.index_age == Some(None) {
            (glyph::WARNING, "Everything isn't running — file results are off. Search \u{201C}restart everything\u{201D}".into(), pal.warn)
        } else if let Some(Some(age)) = self.index_age.filter(|a| a.is_some_and(|s| s > 15 * 60)) {
            (glyph::WARNING, format!("Everything's index looks stuck ({} since the last change)", human_age(now_secs() - age).replace(" ago", "")), pal.warn)
        } else if let Some(f) = self.effective().0 {
            (glyph::FILTER, format!("Showing {} \u{00B7} Ctrl+Tab cycles types \u{00B7} Backspace clears", f.label), pal.faint)
        } else if self.scope.is_some() {
            (glyph::FOLDER, "Searching this folder · Backspace for everywhere".into(), pal.faint)
        } else if self.mode == Mode::Windows {
            (glyph::WINDOW, "Open windows · Backspace for everything".into(), pal.faint)
        } else {
            let total = self.files.first().and_then(|r| r.as_ref().ok()).map_or(0, |r| r.total);
            let text = if self.query.trim().is_empty() || total == 0 {
                "SlothSearch".to_string()
            } else {
                format!("{} files · {} ms", crate::calc::format_num(total as f64, true), self.files_ms)
            };
            // '\0' marks the resting state: the sloth instead of a glyph.
            ('\0', text, pal.faint)
        };
        if g == '\0' {
            let r = Rect::from_center_size(pos2(left + 7.0, cy), vec2(20.0, 20.0));
            p.image(self.sloth_head.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            p.text(pos2(left, cy), Align2::LEFT_CENTER, g.to_string(), icon_font(12.0), color);
        }
        let msg = truncated(&text, regular(12.0), color, rect.width() * 0.55);
        p.galley(pos2(left + 20.0, cy - msg.size().y / 2.0), msg, color);

        // Right: what Enter does, and the actions shortcut.
        let mut x = rect.right() - 16.0;
        if let Some(item) = self.selected_item() {
            x = keycaps(&p, pal, x, cy, &["Ctrl", "K"]);
            let g = p.layout_no_wrap("Actions".into(), regular(12.0), pal.dim);
            x -= g.size().x + 8.0;
            p.galley(pos2(x, cy - g.size().y / 2.0), g, pal.dim);
            x -= 14.0;
            p.vline(x, (cy - 8.0)..=(cy + 8.0), Stroke::new(1.0, pal.divider));
            x -= 12.0;
            let primary = launch::actions(item, self.scope.as_ref()).into_iter().next().map(|a| a.label).unwrap_or_default();
            x = keycaps(&p, pal, x, cy, &["\u{21B5}"]);
            let g = p.layout_no_wrap(primary, semibold(12.0), pal.text);
            x -= g.size().x + 8.0;
            p.galley(pos2(x, cy - g.size().y / 2.0), g, pal.text);
        }
        let _ = x;
    }

    fn actions_menu(&mut self, ui: &mut Ui, panel: Rect, bottom: f32, moved: bool) {
        let pal = self.pal;
        let Some(item) = self.selected_item().cloned() else {
            self.actions_open = false;
            return;
        };
        let acts = self.actions_for(&item);
        let row_h = 34.0;
        let h = 40.0 + acts.len() as f32 * row_h + 8.0;
        let w = 320.0;
        let rect = Rect::from_min_size(pos2(panel.right() - w - 10.0, bottom - h - 8.0), vec2(w, h));
        let p = ui.painter().clone();
        let r = CornerRadius::same(11);
        p.add(Shadow { offset: [0, 8], blur: 28, spread: 0, color: pal.shadow }.as_shape(rect, r));
        let bg = if pal.dark { Color32::from_rgb(40, 40, 46) } else { Color32::from_rgb(255, 255, 255) };
        p.rect_filled(rect, r, bg);
        p.rect_stroke(rect, r, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        let title = truncated(&item.title, semibold(12.0), pal.faint, w - 32.0);
        p.galley(pos2(rect.left() + 16.0, rect.top() + 14.0), title, pal.faint);

        let mut run = None;
        for (i, a) in acts.iter().enumerate() {
            let rr = Rect::from_min_size(pos2(rect.left() + 6.0, rect.top() + 38.0 + i as f32 * row_h), vec2(w - 12.0, row_h));
            let resp = ui.interact(rr, egui::Id::new(("action", i)), Sense::click());
            if resp.hovered() && moved {
                self.action_sel = i;
            }
            if resp.clicked() {
                run = Some(a.act);
            }
            if i == self.action_sel {
                p.rect_filled(rr, 7.0, pal.selected);
            }
            p.text(pos2(rr.left() + 14.0, rr.center().y), Align2::LEFT_CENTER, a.glyph.to_string(), icon_font(13.0), pal.dim);
            p.text(pos2(rr.left() + 38.0, rr.center().y), Align2::LEFT_CENTER, &a.label, regular(13.5), pal.text);
            let parts: Vec<&str> = a.keys.split('+').map(|k| if k == "Enter" { "\u{21B5}" } else { k }).collect();
            keycaps(&p, pal, rr.right() - 10.0, rr.center().y, &parts);
        }
        if let Some(act) = run {
            self.actions_open = false;
            self.run(act, self.selected);
        }
        // Clicking elsewhere in the panel closes the menu.
        if ui.input(|i| i.pointer.any_pressed()) && ui.input(|i| i.pointer.hover_pos()).is_some_and(|pt| !rect.contains(pt)) {
            self.actions_open = false;
        }
    }
}

impl SlothSearch {
    /// The type filter menu: two columns under the filter button.
    fn filter_menu_ui(&mut self, ui: &mut Ui, panel: Rect, moved: bool) {
        let pal = self.pal;
        let mut entries: Vec<(Option<&'static str>, &'static str, char)> = vec![(None, "All types", glyph::SEARCH)];
        entries.extend(filters::all().into_iter().map(|(k, l, g)| (Some(k), l, g)));
        let current = self.effective().0.map(|f| f.key);
        let (cols, row_h, col_w) = (2usize, 30.0, 178.0);
        let rows = entries.len().div_ceil(cols);
        let w = col_w * cols as f32 + 16.0;
        let h = 36.0 + rows as f32 * row_h + 8.0;
        let rect = Rect::from_min_size(pos2(panel.right() - w - 10.0, panel.top() + SEARCH_H - 6.0), vec2(w, h));
        let p = ui.painter().clone();
        let r = CornerRadius::same(11);
        p.add(Shadow { offset: [0, 8], blur: 28, spread: 0, color: pal.shadow }.as_shape(rect, r));
        let bg = if pal.dark { Color32::from_rgb(40, 40, 46) } else { Color32::from_rgb(255, 255, 255) };
        p.rect_filled(rect, r, bg);
        p.rect_stroke(rect, r, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        p.text(pos2(rect.left() + 16.0, rect.top() + 18.0), Align2::LEFT_CENTER, "Filter by type \u{00B7} or type type:pdf", semibold(12.0), pal.faint);

        let mut pick = None;
        for (i, (key, label, g)) in entries.iter().enumerate() {
            // Column-major, so Up/Down walks down a column.
            let (col, row) = (i / rows, i % rows);
            let rr = Rect::from_min_size(
                pos2(rect.left() + 8.0 + col as f32 * col_w, rect.top() + 34.0 + row as f32 * row_h),
                vec2(col_w - 4.0, row_h),
            );
            let resp = ui.interact(rr, egui::Id::new(("filter", i)), Sense::click());
            if resp.hovered() && moved {
                self.filter_sel = i;
            }
            if resp.clicked() {
                pick = Some(i);
            }
            if i == self.filter_sel {
                p.rect_filled(rr, 7.0, pal.selected);
            }
            let on = key.map(str::to_string) == current;
            let fg = if on { pal.accent_text } else { pal.text };
            p.text(pos2(rr.left() + 12.0, rr.center().y), Align2::LEFT_CENTER, g.to_string(), icon_font(12.0), if on { pal.accent_text } else { pal.dim });
            p.text(pos2(rr.left() + 34.0, rr.center().y), Align2::LEFT_CENTER, *label, regular(13.0), fg);
            if on {
                p.text(pos2(rr.right() - 10.0, rr.center().y), Align2::RIGHT_CENTER, '\u{E73E}'.to_string(), icon_font(11.0), pal.accent_text);
            }
        }
        if let Some(i) = pick {
            self.pick_filter(i);
        } else if ui.input(|i| i.pointer.any_pressed())
            && ui.input(|i| i.pointer.hover_pos()).is_some_and(|pt| !rect.contains(pt) && !self.filter_button.contains(pt))
        {
            self.filter_menu = false;
        }
    }
}

/// Draws keycaps right-aligned ending at `right`; returns the left edge.
fn keycaps(p: &egui::Painter, pal: Palette, right: f32, cy: f32, keys: &[&str]) -> f32 {
    let mut x = right;
    for k in keys.iter().rev() {
        let g = p.layout_no_wrap(k.to_string(), regular(11.0), pal.dim);
        let w = (g.size().x + 10.0).max(20.0);
        let r = Rect::from_min_size(pos2(x - w, cy - 10.0), vec2(w, 20.0));
        p.rect_filled(r, 5.0, pal.kbd);
        p.galley(pos2(r.center().x - g.size().x / 2.0, cy - g.size().y / 2.0), g, pal.dim);
        x = r.left() - 4.0;
    }
    x
}

fn glyph_tile(ui: &Ui, g: char, rect: Rect, pal: Palette, accent: bool) {
    let p = ui.painter();
    let (fill, fg) = if accent { (pal.accent.gamma_multiply(0.85), Color32::WHITE) } else { (pal.tile, pal.text) };
    p.rect_filled(rect, rect.width() * 0.25, fill);
    p.text(rect.center(), Align2::CENTER_CENTER, g.to_string(), icon_font(rect.width() * 0.52), fg);
}

fn truncated(text: &str, font: egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = max_w;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    // Painter-independent layout through the global context fonts.
    egui_ctx().fonts_mut(|f| f.layout_job(job))
}

/// The title with matched characters in the accent color.
fn highlighted(title: &str, hl: &[u32], pal: Palette, max_w: f32, big: bool) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    let size = if big { 17.0 } else { 14.5 };
    let mut run = String::new();
    let mut run_hl = false;
    let flush = |job: &mut LayoutJob, run: &mut String, on: bool| {
        if run.is_empty() {
            return;
        }
        let (font, color) = if on { (semibold(size), pal.accent_text) } else { (if big { semibold(size) } else { regular(size) }, pal.text) };
        job.append(run, 0.0, egui::TextFormat { font_id: font, color, ..Default::default() });
        run.clear();
    };
    for (i, ch) in title.chars().enumerate() {
        let on = hl.binary_search(&(i as u32)).is_ok();
        if on != run_hl {
            flush(&mut job, &mut run, run_hl);
            run_hl = on;
        }
        run.push(ch);
    }
    flush(&mut job, &mut run, run_hl);
    job.wrap.max_width = max_w;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    egui_ctx().fonts_mut(|f| f.layout_job(job))
}

fn is_texty(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "md" | "rs" | "toml" | "json" | "jsonc" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "py" | "css" | "scss"
            | "html" | "htm" | "xml" | "yml" | "yaml" | "ini" | "cfg" | "conf" | "log" | "csv" | "tsv" | "ps1" | "psm1" | "sh"
            | "bat" | "cmd" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "java" | "kt" | "go" | "rb" | "php" | "sql" | "env"
            | "gitignore" | "lua" | "swift" | "vue" | "svelte" | "astro" | "prisma" | "graphql" | "gd" | "glsl" | "wgsl" | "hlsl"
            | "srt" | "vtt" | "properties" | "gradle" | "dockerfile" | "makefile" | "cmake" | "tex" | "r" | "dart" | "zig"
    )
}

fn is_thumbable(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "heic" | "ico" | "svg" | "mp4" | "mov" | "mkv"
            | "avi" | "webm" | "wmv" | "m4v" | "pdf" | "psd" | "blend" | "stl" | "3mf" | "glb" | "fbx" | "obj" | "docx"
            | "xlsx" | "pptx"
    )
}
