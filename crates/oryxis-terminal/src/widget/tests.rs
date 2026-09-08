    use super::*;

    fn view_and_state() -> (TerminalView<()>, TerminalWidgetState) {
        let term = TerminalState::new_no_pty(80, 24).unwrap();
        let view = TerminalView::new(Arc::new(Mutex::new(term)));
        (view, TerminalWidgetState::default())
    }

    fn bounds() -> Rectangle {
        Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 480.0))
    }

    /// SGR click tracking (mc, htop). Regression for the "must hold
    /// Shift to click the sidebar" report: a release whose press was
    /// never reported (it landed on a sibling widget, so the cursor is
    /// outside the canvas and no press is tracked) must NOT be consumed
    /// by the report path; capturing it starves sibling `button`s,
    /// which fire on release.
    #[test]
    fn untracked_release_is_not_reported() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let ev = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        // Cursor over the sidebar (outside the canvas), no tracked press.
        let cursor = mouse::Cursor::Available(Point::new(2000.0, 100.0));
        assert!(ws.report_button.is_none());
        let action = view.handle_mouse_report(&mut ws, &ev, bounds(), cursor, mode, 80, 24);
        assert!(action.is_none(), "sidebar release must stay local");
    }

    /// The other half of issue #150: with the remote app holding mouse
    /// tracking (tmux `mouse on`, htop), a high-resolution wheel's
    /// fragments must accumulate into whole detents here too. Reporting
    /// each fragment as a notch — what `ceil()` alone did — scrolled the
    /// remote app eight times per click of the wheel. A residual-only
    /// fragment is still CONSUMED (publishing nothing): while the app
    /// holds tracking the wheel belongs to the report path, and falling
    /// through would hand the fragment to the local-scrollback arm,
    /// which shares the residual and would double-count it.
    #[test]
    fn fractional_line_wheel_reports_one_notch_per_detent() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let view = view.on_terminal_input(|_| ());
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let frag = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 0.125 },
        });

        for _ in 0..7 {
            let action = view
                .handle_mouse_report(&mut ws, &frag, bounds(), cursor, mode, 80, 24)
                .expect("a partial detent is still consumed by the report path");
            let (msg, _, _) = action.into_inner();
            assert!(msg.is_none(), "a partial detent reports nothing");
        }
        let action = view
            .handle_mouse_report(&mut ws, &frag, bounds(), cursor, mode, 80, 24)
            .expect("the completed detent is consumed");
        let (msg, _, _) = action.into_inner();
        assert!(msg.is_some(), "the completed detent reports once");
    }

    /// The touchpad twin: `ScrollDelta::Pixels` fragments arrive a few
    /// pixels at a time, below one cell, and must accumulate on the
    /// cell scale before reporting. Ceiling each fragment to a notch
    /// flooded a tracking TUI with several times the gesture (a slow
    /// two-finger scroll became ~30 wheel reports where ~6 lines were
    /// scrolled), while the same gesture over local scrollback, which
    /// already accumulated, scrolled correctly.
    #[test]
    fn fractional_pixel_wheel_reports_whole_cells_only() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let view = view.on_terminal_input(|_| ());
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        // Four fragments of a quarter-cell each: only the fourth
        // completes a cell and may report.
        let frag = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: view.cell_height / 4.0 },
        });

        for _ in 0..3 {
            let action = view
                .handle_mouse_report(&mut ws, &frag, bounds(), cursor, mode, 80, 24)
                .expect("a sub-cell fragment is still consumed by the report path");
            let (msg, _, _) = action.into_inner();
            assert!(msg.is_none(), "a sub-cell fragment reports nothing");
        }
        let action = view
            .handle_mouse_report(&mut ws, &frag, bounds(), cursor, mode, 80, 24)
            .expect("the completed cell is consumed");
        let (msg, _, _) = action.into_inner();
        assert!(msg.is_some(), "the completed cell reports once");
    }

    /// The canvas-originated press → drag off-canvas → release flow must
    /// still report the release (apps need the button-up to end a drag),
    /// falling back to the last reported cell.
    #[test]
    fn tracked_release_still_reports_after_leaving_canvas() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let inside = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let action = view.handle_mouse_report(&mut ws, &press, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "on-canvas press must be reported");
        assert_eq!(ws.report_button, Some(ReportButton::Left));

        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let outside = mouse::Cursor::Available(Point::new(2000.0, 100.0));
        let action = view.handle_mouse_report(&mut ws, &release, bounds(), outside, mode, 80, 24);
        assert!(action.is_some(), "release of a reported press must land");
        assert!(ws.report_button.is_none(), "press tracking cleared on release");
    }

    /// Pressing Shift AFTER a reported press must not swallow the
    /// release: `release_completes_tracked_press` lets it through the
    /// Shift bypass, so the app gets its button-up and `report_button`
    /// clears instead of sticking at `Some(Left)` (phantom held button,
    /// every later motion misread as a drag).
    #[test]
    fn shift_at_release_does_not_swallow_tracked_release() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let inside = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let action = view.handle_mouse_report(&mut ws, &press, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "press without Shift must be reported");
        assert_eq!(ws.report_button, Some(ReportButton::Left));

        // Shift lands between press and release.
        ws.modifiers = iced::keyboard::Modifiers::SHIFT;
        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        assert!(
            TerminalView::<()>::release_completes_tracked_press(&ws, &release),
            "tracked release must pierce the Shift bypass"
        );
        let action = view.handle_mouse_report(&mut ws, &release, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "release of a tracked press reports despite Shift");
        assert!(ws.report_button.is_none(), "press tracking cleared on release");
    }

    /// The Shift bypass must keep blocking NEW gestures: with no
    /// tracked press, neither a Shift+press nor its release qualifies
    /// as completing a tracked press, so local selection stays in
    /// charge for the whole gesture.
    #[test]
    fn shift_bypass_still_blocks_new_gestures() {
        let (_view, ws) = view_and_state();
        assert!(ws.report_button.is_none());
        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        assert!(
            !TerminalView::<()>::release_completes_tracked_press(&ws, &press),
            "a press never qualifies"
        );
        assert!(
            !TerminalView::<()>::release_completes_tracked_press(&ws, &release),
            "a release with no tracked press never qualifies"
        );
    }

    /// `right_click_copy` is a Paste-scheme sub-option: a stale `true`
    /// under Menu / Extend (Settings hides the toggle there, so the
    /// user can't see or clear it) must not defer, i.e. suppress, the
    /// copy-on-select auto-copy.
    #[test]
    fn right_click_copy_only_defers_auto_copy_under_paste_scheme() {
        let (view, _) = view_and_state();
        let paste = view.with_right_click_copy(true).with_right_click_action(RightClickAction::Paste);
        assert!(paste.defers_copy_to_right_click(), "Paste scheme honours the deferral");

        let (view, _) = view_and_state();
        let menu = view.with_right_click_copy(true).with_right_click_action(RightClickAction::Menu);
        assert!(!menu.defers_copy_to_right_click(), "stale flag under Menu must not defer");

        let (view, _) = view_and_state();
        let extend = view.with_right_click_copy(true).with_right_click_action(RightClickAction::Extend);
        assert!(!extend.defers_copy_to_right_click(), "stale flag under Extend must not defer");

        let (view, _) = view_and_state();
        let off = view.with_right_click_action(RightClickAction::Paste);
        assert!(!off.defers_copy_to_right_click(), "flag off never defers");
    }

    /// Build a view over a terminal with `lines` rows of scrollback, so
    /// there is somewhere to scroll to.
    fn scrolled_view(lines: usize) -> (TerminalView<()>, TerminalWidgetState) {
        let mut term = TerminalState::new_no_pty(80, 24).unwrap();
        for _ in 0..lines {
            term.process(b"line\r\n");
        }
        (
            TerminalView::new(Arc::new(Mutex::new(term))),
            TerminalWidgetState::default(),
        )
    }

    /// A scrolled-back terminal driven only by `ScrollDelta::Pixels`
    /// deltas smaller than one cell (Windows precision touchpads and
    /// high-res wheels deliver a few pixels per notch): the pre-#91
    /// handler floored each `y / cell_height` to zero, so scrollback
    /// never moved and the transcript viewer (no output to snap it back)
    /// was frozen. The residual accumulator now carries the sub-cell
    /// remainder across events and emits a whole line once the pixels
    /// cross a cell.
    #[test]
    fn subcell_pixel_wheel_accumulates_into_scroll() {
        let (view, mut ws) = scrolled_view(200);
        // Cursor over the canvas; start at the live edge (offset 0).
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        assert_eq!(ws.scroll_offset.get(), 0);

        // cell_height defaults to 14.0 * 1.15 = 16.1, so a 10px notch is
        // sub-cell: one alone must not move (correct), but the second
        // crosses a cell boundary and advances exactly one line, where
        // the old truncation stayed pinned at zero forever.
        let notch = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: 10.0 },
        });
        let action = view.on_event(&mut ws, &notch, bounds(), cursor);
        assert!(action.is_some(), "the canvas consumes the wheel event");
        assert_eq!(ws.scroll_offset.get(), 0, "one sub-cell notch must not move");

        view.on_event(&mut ws, &notch, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 1, "two sub-cell notches cross a cell");

        // Five more keep it climbing, proving the residual never stalls.
        for _ in 0..5 {
            view.on_event(&mut ws, &notch, bounds(), cursor);
        }
        assert!(
            ws.scroll_offset.get() >= 4,
            "sub-cell pixel wheel keeps advancing, got {}",
            ws.scroll_offset.get()
        );
    }

    /// A `ScrollDelta::Lines` notch still moves whole lines and clears
    /// any carried pixel residual, so switching devices (touchpad →
    /// discrete wheel) can't leave a stale sub-cell fraction fighting the
    /// next notch.
    #[test]
    fn line_wheel_moves_and_clears_pixel_residual() {
        let (view, mut ws) = scrolled_view(200);
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));

        // Leave a sub-cell residual behind from a pixel notch.
        let px = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: 10.0 },
        });
        view.on_event(&mut ws, &px, bounds(), cursor);
        assert_ne!(ws.scroll_px_residual.get(), 0.0, "pixel notch left a residual");

        // A line notch scrolls 3 lines (y * 3) and wipes the residual.
        let ln = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
        });
        view.on_event(&mut ws, &ln, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 3, "one line notch scrolls 3 lines");
        assert_eq!(ws.scroll_px_residual.get(), 0.0, "line notch clears the residual");
    }

    /// A high-resolution wheel reports FRACTIONS of a detent, and the
    /// platform hands them through as `ScrollDelta::Lines` on the same
    /// 120-per-detent scale it already divided out: Wayland's
    /// `axis_value120` (which winit only started honouring once the
    /// toolkit began binding `wl_seat` v9, in the 0.31 bump that shipped
    /// with 0.13.0) and Windows' `WM_MOUSEWHEEL`. `y as i32` truncated
    /// every fragment to zero and then swallowed the event, so the wheel
    /// did nothing at all on those devices (issue #150). The notch
    /// residual accumulates them into whole detents instead.
    #[test]
    fn fractional_line_wheel_accumulates_into_scroll() {
        let (view, mut ws) = scrolled_view(200);
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        assert_eq!(ws.scroll_offset.get(), 0);

        // An eighth of a detent, the `value120 = 15` fragment.
        let frag = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 0.125 },
        });
        for _ in 0..7 {
            let action = view.on_event(&mut ws, &frag, bounds(), cursor);
            assert!(action.is_some(), "the canvas consumes every fragment");
        }
        assert_eq!(ws.scroll_offset.get(), 0, "a partial detent must not move");

        // The eighth fragment completes the detent: 3 lines, once.
        view.on_event(&mut ws, &frag, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 3, "a whole detent scrolls 3 lines");

        // And it keeps going, which is what the truncation never did.
        for _ in 0..8 {
            view.on_event(&mut ws, &frag, bounds(), cursor);
        }
        assert_eq!(ws.scroll_offset.get(), 6, "the residual never stalls");
    }

    /// A direction reversal mid-detent responds on its first fragment
    /// instead of spending it unwinding the accumulated one; a
    /// horizontal-only event (a tilt wheel, `y == 0.0`) is NOT a
    /// reversal and must leave the vertical residual alone.
    #[test]
    fn fractional_line_wheel_reversal_and_tilt() {
        let (view, mut ws) = scrolled_view(200);
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let wheel = |y: f32, x: f32| {
            iced::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x, y },
            })
        };

        // Half a detent up, then a tilt: the residual survives it.
        view.on_event(&mut ws, &wheel(0.5, 0.0), bounds(), cursor);
        view.on_event(&mut ws, &wheel(0.0, 1.0), bounds(), cursor);
        assert_eq!(ws.scroll_line_residual.get(), 0.5, "a tilt is not a reversal");

        // Reversing drops the stale residual, so the next half-detent
        // down is a fresh 0.5 rather than cancelling back to zero.
        view.on_event(&mut ws, &wheel(-0.5, 0.0), bounds(), cursor);
        assert_eq!(ws.scroll_line_residual.get(), -0.5, "a reversal starts over");
    }

    /// Regression: while the viewport is held (scrolled up), new output
    /// rotates rows into history and the grid raises its own offset so the
    /// same rows stay on screen. The selection is stored in raw grid-line
    /// coordinates, which drop by exactly that drift, so the draw pass must
    /// translate the stored range or the highlight band slides one row per
    /// line of output while the pinned text stays put.
    #[test]
    fn selection_follows_content_when_output_runs_under_a_held_viewport() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 100).unwrap(),
        ));
        for i in 0..7 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        // Five lines of history (the trailing CRLF after l6 scrolled it
        // into history), the screen shows l5 l6 + blank. Scroll up three
        // rows so the viewport shows l2 l3 l4 (raw grid lines -3..=-1) and
        // plant a selection over exactly those rows (base == mirror == 3).
        term.lock().unwrap().scroll_viewport_by(3);
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let ws = TerminalWidgetState::default();
        ws.selection.set(Some(Selection { start: (0, -3), end: (1, -1), block: false }));
        ws.selection_base.set(3);
        ws.scroll_offset.set(3);

        // Two more lines land while the user is scrolled up: the grid
        // raises its offset to 5 so l2 l3 l4 stay on the same rows.
        term.lock().unwrap().process(b"l7\r\nl8\r\n");
        assert_eq!(term.lock().unwrap().viewport_offset(), 5, "the viewport stays put");

        // Without translation the stale range (-3..=-1) now covers l4 l5 l6:
        // the band sliding one row per output line is exactly the bug.
        {
            let s = term.lock().unwrap();
            let stale = Selection { start: (0, -3), end: (1, -1), block: false };
            assert_eq!(s.get_selection_text(&stale), "l4\nl5\nl6", "stale coords drift");
        }

        // The draw pass catches the selection up against the grid's offset
        // before painting.
        {
            let s = term.lock().unwrap();
            let offset = s.viewport_offset();
            view.rebase_selection(&ws, offset);
        }
        let sel = ws.selection.get().expect("still selected");
        assert_eq!((sel.start.1, sel.end.1), (-5, -3), "raw lines follow the rotation");
        let text = term.lock().unwrap().get_selection_text(&sel);
        assert_eq!(text, "l2\nl3\nl4", "the band stays on the selected content");
    }

    /// A manual viewport move (wheel, scrollbar, page keys) must NOT
    /// translate a stored selection: it changes what rows are visible, not
    /// where the content is, so the band has to ride the same raw lines.
    /// Only grid rotation (output under a held viewport) translates them.
    #[test]
    fn selection_is_not_translated_by_manual_viewport_scrolls() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 1000).unwrap(),
        ));
        for i in 0..60 {
            term.lock().unwrap().process(format!("line {i}\r\n").as_bytes());
        }
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let mut ws = TerminalWidgetState::default();
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let wheel = |y: f32| iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y },
        });

        // One notch = three lines of scrollback.
        view.on_event(&mut ws, &wheel(1.0), bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 3, "one notch scrolls three lines");

        // A selection made while looking at rows offset 3: raw lines -3..-1.
        let sel = Selection { start: (0, -3), end: (1, -1), block: false };
        ws.selection.set(Some(sel));
        ws.selection_base.set(ws.scroll_offset.get());
        let at_3 = term.lock().unwrap().get_selection_text(&sel);

        // Wheel down to the live edge and back up: content never moved, so
        // the stored lines must be untouched (and the base must ride along
        // so the next rotation is measured from where the viewport is).
        view.on_event(&mut ws, &wheel(-1.0), bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 0, "wheel down reaches the live edge");
        view.on_event(&mut ws, &wheel(1.0), bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 3, "wheel up returns to offset 3");

        let sel = ws.selection.get().expect("selection survives the scrolls");
        assert_eq!((sel.start.1, sel.end.1), (-3, -1), "a view move must not translate it");
        assert_eq!(ws.selection_base.get(), 3, "the base rides the viewport");
        assert_eq!(
            term.lock().unwrap().get_selection_text(&sel),
            at_3,
            "same content under the selection after scrolling away and back"
        );

        // Now output lands under the held viewport: the grid raises its
        // offset (3 -> 5) and the draw's rebase translates the band with it.
        term.lock().unwrap().process(b"line 60\r\nline 61\r\n");
        {
            let s = term.lock().unwrap();
            assert_eq!(s.viewport_offset(), 5, "rows held while output runs");
            view.rebase_selection(&ws, 5);
        }
        let sel = ws.selection.get().expect("still selected");
        assert_eq!((sel.start.1, sel.end.1), (-5, -3), "rotation translates the band");
        assert_eq!(
            term.lock().unwrap().get_selection_text(&sel),
            at_3,
            "the band stays on the same content while output runs"
        );
    }

    /// The PRIMARY ghost band is a completed selection demoted to a faint
    /// reminder, stored in the same raw-line space as a live band. Grid
    /// rotation under a held viewport must translate it in place too — once
    /// the scrollback is full, `total_lines` stops growing and the draw's
    /// guard can no longer tell the rows moved, so the ghost would slide
    /// like the pre-fix live band did.
    #[test]
    fn ghost_follows_rotation_like_a_live_band() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 1000).unwrap(),
        ));
        for i in 0..7 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        term.lock().unwrap().scroll_viewport_by(3);
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let ws = TerminalWidgetState::default();
        ws.selection.set(None);
        // Ghost captured while the viewport sat at offset 3 over l2..l4.
        ws.primary_ghost.set(Some((
            Selection { start: (0, -3), end: (1, -1), block: false },
            24,
            8,
        )));
        ws.selection_base.set(3);

        // Two lines of output rotate the rows under the held viewport
        // (offset 3 -> 5); the rebase the draw pass runs must translate the
        // ghost with the content, exactly like the live band.
        term.lock().unwrap().process(b"l7\r\nl8\r\n");
        {
            let s = term.lock().unwrap();
            assert_eq!(s.viewport_offset(), 5, "rows held while output runs");
            view.rebase_selection(&ws, 5);
        }
        let ghost = ws.primary_ghost.get().expect("ghost kept");
        assert_eq!(
            (ghost.0.start.1, ghost.0.end.1),
            (-5, -3),
            "ghost lines follow the rotation"
        );
        assert_eq!(
            term.lock().unwrap().get_selection_text(&ghost.0),
            "l2\nl3\nl4",
            "the ghost stays on the content it was captured from"
        );

        // A viewport-only move re-anchors the base but must not translate
        // the ghost's stored lines.
        ws.set_viewport_offset(0);
        view.rebase_selection(&ws, 0);
        let ghost = ws.primary_ghost.get().expect("ghost kept");
        assert_eq!(
            (ghost.0.start.1, ghost.0.end.1),
            (-5, -3),
            "a view move must not move the ghost's raw lines"
        );
    }

    /// The grid geometry `(columns, screen_lines, total_lines)` plus the
    /// monotonic scroll counter — everything the draw pass's upkeep
    /// compares against the stored selection state each frame.
    fn grid_dims(term: &Arc<Mutex<TerminalState>>) -> (u16, u16, i32, usize) {
        use alacritty_terminal::grid::Dimensions;
        let s = term.lock().unwrap();
        let g = s.backend.term.grid();
        (
            g.columns() as u16,
            g.screen_lines() as u16,
            g.total_lines() as i32,
            g.scrolled_lines(),
        )
    }

    /// The live edge has no offset drift to follow: the viewport sits at 0
    /// and output simply pushes rows past the bottom of the screen, so the
    /// draw pass has to translate the band by the grid's monotonic scroll
    /// counter or the highlight stays frozen on the same SCREEN rows while
    /// the text it marked scrolls away under it.
    #[test]
    fn upkeep_follows_the_output_at_the_live_edge() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 100).unwrap(),
        ));
        for i in 0..6 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let mut ws = TerminalWidgetState::default();
        let (cols, rows, total, scrolled) = grid_dims(&term);

        // A band over the first two screen rows, at the live edge.
        let sel = Selection { start: (0, 0), end: (1, 1), block: false };
        let marked = term.lock().unwrap().get_selection_text(&sel);
        ws.selection.set(Some(sel));
        ws.select_anchor.set(Some((SelectGranularity::Word, (0, 0))));
        ws.primary_ghost.set(Some((sel, cols, total as usize)));
        ws.selection_base.set(0);
        ws.last_geom.set((cols, rows, total));
        ws.last_scrolled.set(scrolled);
        ws.sel_present_last_draw.set(true);

        // Output at the edge: the viewport does not move, the content does.
        term.lock().unwrap().process(b"l6\r\nl7\r\n");
        assert_eq!(term.lock().unwrap().viewport_offset(), 0, "the edge holds");
        let (cols2, rows2, total2, scrolled2) = grid_dims(&term);
        let moved = scrolled2 - scrolled;
        assert!(moved > 0, "the counter advanced while the rows rotated");
        view.upkeep_selection_for_draw(&ws, cols2, rows2, total2, scrolled2, false);

        let sel = ws.selection.get().expect("the band survives");
        assert_eq!(
            (sel.start.1, sel.end.1),
            (-(moved as i32), 1 - moved as i32),
            "raw lines ride the rotation"
        );
        assert_eq!(
            term.lock().unwrap().get_selection_text(&sel),
            marked,
            "the highlight stays on the text it marked"
        );
        let (_, (_acol, aline)) = ws.select_anchor.get().expect("anchor kept");
        assert_eq!(aline, -(moved as i32), "a held drag anchor rides with it");
        assert_eq!(ws.selection_base.get(), 0, "still measured from the edge");
        // The ghost is translated too, but its capture-time `total` guard
        // still says stale (the total grew), so it is not drawn.
        let ghost = ws.primary_ghost.get().expect("ghost kept");
        assert_eq!(
            (ghost.0.start.1, ghost.0.end.1),
            (-(moved as i32), 1 - moved as i32)
        );
        assert_eq!(ghost.2, total as usize, "guard still says stale");

        // Mid-drag the upkeep translates too: between two motion events a
        // band that is not moved with the content would slide off the rows
        // the pointer is actually over (the next motion rewrites the end
        // from the pointer, so there is no double-apply).
        let dragging = ws.selection.get().unwrap();
        ws.selecting = true;
        term.lock().unwrap().process(b"l8\r\n");
        let (cols3, rows3, total3, scrolled3) = grid_dims(&term);
        view.upkeep_selection_for_draw(&ws, cols3, rows3, total3, scrolled3, false);
        let sel = ws.selection.get().unwrap();
        assert_eq!(
            sel.start.1,
            dragging.start.1 - (scrolled3 - scrolled2) as i32,
            "a mid-drag rotation rides the band too"
        );
    }

    /// With the scrollback FULL, `total_lines` stops moving and the raised
    /// `display_offset` only follows the rotation while the viewport is
    /// pinned below the very top. The grid's monotonic scroll counter is
    /// the one signal that keeps counting either way, so the upkeep must
    /// translate the band by IT — this is the regression guard for that.
    #[test]
    fn upkeep_translates_a_held_band_once_the_scrollback_is_full() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 5).unwrap(),
        ));
        for i in 0..10 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        assert_eq!(
            term.lock().unwrap().backend.term.grid().history_size(),
            5,
            "the cap is reached before anything is selected"
        );
        term.lock().unwrap().scroll_viewport_by(3);
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let ws = TerminalWidgetState::default();
        let (cols, rows, total, scrolled) = grid_dims(&term);
        let offset = term.lock().unwrap().viewport_offset();
        let sel = Selection { start: (0, -3), end: (1, -1), block: false };
        let marked = term.lock().unwrap().get_selection_text(&sel);
        ws.selection.set(Some(sel));
        ws.selection_base.set(offset);
        ws.scroll_offset.set(offset);
        ws.last_geom.set((cols, rows, total));
        ws.last_scrolled.set(scrolled);
        ws.sel_present_last_draw.set(true);

        // Two lines of output at the cap: the total cannot grow, only the
        // monotonic counter can.
        term.lock().unwrap().process(b"l10\r\nl11\r\n");
        let (cols2, rows2, total2, scrolled2) = grid_dims(&term);
        assert_eq!(total2, total, "the cap hides the rotation from the total");
        assert_eq!(scrolled2 - scrolled, 2, "the counter does not stop at the cap");

        view.upkeep_selection_for_draw(&ws, cols2, rows2, total2, scrolled2, false);
        let sel = ws.selection.get().expect("the band survives");
        assert_eq!(
            term.lock().unwrap().get_selection_text(&sel),
            marked,
            "the highlight stays on the text it marked"
        );
    }

    /// The scrollback cap used to be a dead end at the live edge: the
    /// offset stays 0 and `total_lines` freezes once the cap is hit, so
    /// neither signal can see the rows keep rotating. The monotonic counter
    /// (the alacritty patch) is the one that still can — this is the exact
    /// scenario the patch exists for.
    #[test]
    fn upkeep_follows_at_the_live_edge_once_the_scrollback_is_full() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 5).unwrap(),
        ));
        for i in 0..10 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        assert_eq!(term.lock().unwrap().backend.term.grid().history_size(), 5);
        assert_eq!(term.lock().unwrap().viewport_offset(), 0, "at the live edge");
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let ws = TerminalWidgetState::default();
        let (cols, rows, total, scrolled) = grid_dims(&term);

        // A band over the first two screen rows, at the live edge.
        let sel = Selection { start: (0, 0), end: (1, 1), block: false };
        let marked = term.lock().unwrap().get_selection_text(&sel);
        ws.selection.set(Some(sel));
        ws.selection_base.set(0);
        ws.last_geom.set((cols, rows, total));
        ws.last_scrolled.set(scrolled);
        ws.sel_present_last_draw.set(true);

        // Output at the edge: the total is pinned at the cap, only the
        // monotonic counter moves.
        term.lock().unwrap().process(b"l10\r\nl11\r\n");
        assert_eq!(term.lock().unwrap().viewport_offset(), 0, "still the live edge");
        let (cols2, rows2, total2, scrolled2) = grid_dims(&term);
        assert_eq!(total2, total, "the total froze at the cap");
        assert_eq!(scrolled2 - scrolled, 2, "the counter did not freeze");

        view.upkeep_selection_for_draw(&ws, cols2, rows2, total2, scrolled2, false);
        let sel = ws.selection.get().expect("the band survives");
        assert_eq!(
            (sel.start.1, sel.end.1),
            (-2, -1),
            "raw lines rode the rotation the total could not see"
        );
        assert_eq!(
            term.lock().unwrap().get_selection_text(&sel),
            marked,
            "the band stays on its text across a full scrollback"
        );
    }

    /// A column change re-wraps the buffer: no translation can put a stored
    /// band back on its text (alacritty and xterm lose the highlight on a
    /// reflow for the same reason), so a band that predates the change is
    /// dropped rather than painted over unrelated cells. One made AFTER it
    /// is already in the new layout and must survive.
    #[test]
    fn upkeep_drops_a_band_that_predates_a_reflow() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 100).unwrap(),
        ));
        for i in 0..6 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let ws = TerminalWidgetState::default();
        let (cols, rows, total, scrolled) = grid_dims(&term);
        let sel = Selection { start: (0, 0), end: (1, 1), block: false };
        ws.selection.set(Some(sel));
        ws.select_anchor.set(Some((SelectGranularity::Word, (0, 0))));
        ws.primary_ghost.set(Some((sel, cols, total as usize)));
        ws.last_geom.set((cols, rows, total));
        ws.last_scrolled.set(scrolled);
        ws.sel_present_last_draw.set(true);

        // A narrower pane reindexes every line.
        term.lock().unwrap().resize(12, 3);
        let (cols2, rows2, total2, scrolled2) = grid_dims(&term);
        assert_ne!(cols2, cols, "the reflow is what the draw sees");
        view.upkeep_selection_for_draw(&ws, cols2, rows2, total2, scrolled2, false);
        assert!(ws.selection.get().is_none(), "a reflow drops the band");
        assert!(ws.select_anchor.get().is_none(), "and the drag anchor");
        assert!(ws.primary_ghost.get().is_none(), "and the ghost that predates it");
        assert!(!ws.sel_present_last_draw.get());

        // A band made after the change is in the new layout: the next
        // frame's upkeep must leave it alone.
        ws.selection.set(Some(Selection { start: (0, 0), end: (1, 0), block: false }));
        ws.sel_present_last_draw.set(true);
        view.upkeep_selection_for_draw(&ws, cols2, rows2, total2, scrolled2, false);
        assert_eq!(
            ws.selection.get(),
            Some(Selection { start: (0, 0), end: (1, 0), block: false }),
            "a band made after the reflow survives it"
        );
    }

    /// The alternate screen is a grid of its own, so the raw lines a band
    /// names no longer refer to what is on screen: it goes, like on a
    /// reflow. The ghost is different — it remembers a selection made in
    /// the MAIN grid, which the alt app only covers — so it waits the
    /// stint out (the draw pass already hides it there) and is eligible
    /// again the moment the app quits.
    #[test]
    fn upkeep_drops_a_band_across_an_alt_screen_switch() {
        let term = Arc::new(Mutex::new(
            TerminalState::new_no_pty_with_scrollback(24, 3, 100).unwrap(),
        ));
        for i in 0..6 {
            term.lock().unwrap().process(format!("l{i}\r\n").as_bytes());
        }
        let view: TerminalView<()> = TerminalView::new(Arc::clone(&term));
        let ws = TerminalWidgetState::default();
        let (cols, rows, total, scrolled) = grid_dims(&term);
        let sel = Selection { start: (0, 0), end: (1, 1), block: false };
        ws.selection.set(Some(sel));
        ws.select_anchor.set(Some((SelectGranularity::Word, (0, 0))));
        ws.primary_ghost.set(Some((sel, cols, total as usize)));
        ws.last_geom.set((cols, rows, total));
        ws.last_scrolled.set(scrolled);
        ws.sel_present_last_draw.set(true);

        // A full-screen app takes over. The alt grid has its own (young)
        // scroll counter; the flip itself drops the band, so the counter
        // jump must not translate anything.
        term.lock().unwrap().process(b"\x1b[?1049h\x1b[Happ frame");
        let (acols, arows, atotal, ascrolled) = grid_dims(&term);
        view.upkeep_selection_for_draw(&ws, acols, arows, atotal, ascrolled, true);
        assert!(ws.selection.get().is_none(), "the alt grid is not the band's grid");
        assert!(ws.select_anchor.get().is_none());
        let ghost = ws.primary_ghost.get().expect("the ghost waits it out");
        assert_eq!((ghost.0.start.1, ghost.0.end.1), (0, 1), "unmoved");

        // The app quits: the main grid comes back exactly as it was, so the
        // counter jumping back must NOT read as rotation, and the ghost's
        // own guards match again.
        term.lock().unwrap().process(b"\x1b[?1049l");
        let (cols2, rows2, total2, scrolled2) = grid_dims(&term);
        assert_eq!(total2, total, "the primary buffer was untouched");
        ws.selection.set(Some(sel));
        ws.sel_present_last_draw.set(true);
        view.upkeep_selection_for_draw(&ws, cols2, rows2, total2, scrolled2, false);
        assert!(ws.selection.get().is_none(), "it belonged to the alt grid");
        let ghost = ws.primary_ghost.get().expect("it survived the round trip");
        assert_eq!((ghost.0.start.1, ghost.0.end.1), (0, 1), "still on its content");
        assert_eq!(ghost.2, total2 as usize, "eligible for the draw again");
    }

    /// `screen_as_ansi` must reproduce the visible screen when fed to a
    /// fresh emulator: text, named / indexed / RGB colors, wide (CJK)
    /// glyphs and the visual attribute flags all round-trip cell-exact.
    /// Backs the transcript viewer's final-alt-frame materialization.
    #[test]
    fn screen_as_ansi_roundtrips_the_visible_screen() {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags as CellFlags;

        let mut a = TerminalState::new_no_pty(24, 7).unwrap();
        a.process(b"\x1b[31mred\x1b[0m plain\r\n");
        a.process(b"\x1b[1;44mB on blue\x1b[0m\r\n");
        a.process(b"\x1b[38;2;1;2;3mrgb\x1b[0m \x1b[7minv\x1b[0m \x1b[38;5;123midx\x1b[0m\r\n");
        a.process("wide 漢字 ok\r\n".as_bytes());
        // A background bar with trailing colored blanks (the top header
        // pattern) followed by default-styled text.
        a.process(b"\x1b[4mu\x1b[0m\x1b[42m  \x1b[0mtail\r\n");
        // Every underline variant, plus an RGB underline color: each
        // must keep its exact style through the round-trip, not reduce
        // to plain underline.
        a.process(b"\x1b[4:2md\x1b[0m\x1b[4:3mc\x1b[0m\x1b[4:4mo\x1b[0m\x1b[4:5ma\x1b[0m\r\n");
        a.process(b"\x1b[4m\x1b[58;2;10;20;30mUC\x1b[0m");

        let bytes = a.screen_as_ansi();
        let mut b = TerminalState::new_no_pty(24, 7).unwrap();
        b.process(&bytes);

        let style = CellFlags::INVERSE
            | CellFlags::BOLD
            | CellFlags::ITALIC
            | CellFlags::DIM
            | CellFlags::HIDDEN
            | CellFlags::STRIKEOUT
            | CellFlags::ALL_UNDERLINES;
        let ga = a.backend.term.grid();
        let gb = b.backend.term.grid();
        assert_eq!(ga.screen_lines(), gb.screen_lines());
        for r in 0..ga.screen_lines() as i32 {
            for c in 0..ga.columns() {
                let ca = &ga[Line(r)][Column(c)];
                let cb = &gb[Line(r)][Column(c)];
                let norm = |ch: char| if ch == '\0' { ' ' } else { ch };
                assert_eq!(norm(ca.c), norm(cb.c), "char at {r},{c}");
                assert_eq!(ca.fg, cb.fg, "fg at {r},{c}");
                assert_eq!(ca.bg, cb.bg, "bg at {r},{c}");
                assert_eq!(ca.flags & style, cb.flags & style, "flags at {r},{c}");
                assert_eq!(
                    ca.underline_color(),
                    cb.underline_color(),
                    "underline color at {r},{c}"
                );
            }
        }
    }

    /// A surface rendered unfocused BY CONSTRUCTION keeps its selection.
    /// The session player replays into such a widget (its keys are
    /// transport controls, so it never takes focus), and while the
    /// lose-focus sweep tested `!focused` alone, the first mouse motion
    /// of the drag that made a selection wiped it: nothing in a
    /// recording could be selected, let alone copied.
    #[test]
    fn a_never_focused_surface_keeps_its_selection() {
        let (view, mut ws) = view_and_state();
        let view = view.focused(false);
        ws.selection
            .set(Some(Selection { start: (0, 0), end: (5, 0), block: false }));
        let ev = iced::Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(40.0, 40.0),
        });
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        view.on_event(&mut ws, &ev, bounds(), cursor);
        assert!(
            ws.selection.get().is_some(),
            "a display-only surface never had focus to lose"
        );
    }

    /// The other half of the same rule: a pane that WAS focused drops its
    /// highlight once it isn't. Split a tab three ways and every pane you
    /// ever selected in would otherwise stay lit, with nothing saying
    /// which one the next copy takes.
    #[test]
    fn a_pane_that_loses_focus_drops_its_highlight() {
        let term = Arc::new(Mutex::new(TerminalState::new_no_pty(80, 24).unwrap()));
        let focused: TerminalView<()> = TerminalView::new(Arc::clone(&term)).focused(true);
        let unfocused: TerminalView<()> = TerminalView::new(term).focused(false);
        let mut ws = TerminalWidgetState::default();
        ws.selection
            .set(Some(Selection { start: (0, 0), end: (5, 0), block: false }));
        let ev = iced::Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(40.0, 40.0),
        });
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));

        focused.on_event(&mut ws, &ev, bounds(), cursor);
        assert!(ws.selection.get().is_some(), "the focused pane keeps its highlight");

        unfocused.on_event(&mut ws, &ev, bounds(), cursor);
        assert!(ws.selection.get().is_none(), "the pane being left drops its highlight");
    }

    /// A key event carrying `key`, with no modifiers: enough for the
    /// chord arm, which resolves through the app's matcher rather than
    /// reading the modifiers itself.
    fn key_press(key: keyboard::Key) -> iced::Event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key.clone(),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }

    /// The session player renders its stage unfocused (its keys are the
    /// transport, so it must not take the "typing clears the highlight"
    /// path), which used to mean its chords never fired either: a
    /// recording could be selected with the mouse but not copied from
    /// the keyboard. `with_chords_unfocused` is the opt-in for a surface
    /// that is the only terminal on screen.
    #[test]
    fn chords_fire_unfocused_when_the_surface_opts_in() {
        let (view, mut ws) = view_and_state();
        let view = view
            .focused(false)
            .with_terminal_chords(Box::new(|_, _| Some(TerminalChordAction::SelectAll)))
            .with_chords_unfocused(true);
        view.on_event(
            &mut ws,
            &key_press(keyboard::Key::Character("a".into())),
            bounds(),
            mouse::Cursor::Unavailable,
        );
        assert!(ws.selection.get().is_some(), "select-all must reach the replay");
    }

    /// The other side of the same gate: without the opt-in an unfocused
    /// widget ignores the chords. Key events reach every widget in the
    /// tree, so this is what keeps a three-way split from running the
    /// copy chord three times.
    #[test]
    fn chords_stay_focus_gated_by_default() {
        let (view, mut ws) = view_and_state();
        let view = view
            .focused(false)
            .with_terminal_chords(Box::new(|_, _| Some(TerminalChordAction::SelectAll)));
        view.on_event(
            &mut ws,
            &key_press(keyboard::Key::Character("a".into())),
            bounds(),
            mouse::Cursor::Unavailable,
        );
        assert!(ws.selection.get().is_none(), "an unfocused pane declines the chord");
    }

    /// Everything a dead session can leave armed, in one pane, so the
    /// reset is asserted against the state it exists for.
    fn state_with_stale_modes() -> TerminalState {
        let mut term = TerminalState::new_no_pty(80, 24).unwrap();
        // What a killed tmux / vim leaves behind: any-motion tracking
        // (1003) with both encodings (1005/1006), focus reporting (1004),
        // bracketed paste (2004), application cursor keys (1), autowrap
        // off (7) and a hidden cursor (25).
        term.process(b"\x1b[?1;1003;1004;1005;1006;2004h\x1b[?7;25l");
        term
    }

    /// The reset the app feeds on disconnect and on every fresh session
    /// (`SESSION_MODE_RESET`) must clear every mode the widget's
    /// mouse-report gate reads. Guard for the reconnect-garbage bug: stale
    /// 1000/1002/1003/1006 left by a dead session made the widget keep
    /// synthesizing SGR reports into a shell that never asked for them,
    /// and the shell's echo of those reports landed on screen as text.
    /// Regression at the gate level: with the modes cleared, a pointer
    /// move must produce NO report.
    #[test]
    fn session_reset_clears_mouse_tracking_and_blocks_reports() {
        use alacritty_terminal::term::TermMode;

        let mut term = state_with_stale_modes();
        assert!(
            term.backend.term.mode().intersects(TermMode::MOUSE_MODE),
            "precondition: stale mouse tracking armed"
        );

        term.process(crate::SESSION_MODE_RESET);

        let mode = *term.backend.term.mode();
        assert!(
            !mode.intersects(TermMode::MOUSE_MODE),
            "mouse tracking cleared"
        );
        assert!(!mode.contains(TermMode::SGR_MOUSE), "SGR encoding cleared");
        assert!(!mode.contains(TermMode::UTF8_MOUSE), "UTF-8 encoding cleared");
        assert!(
            !mode.contains(TermMode::FOCUS_IN_OUT),
            "focus reporting cleared"
        );
        assert!(
            !mode.contains(TermMode::BRACKETED_PASTE),
            "bracketed paste cleared"
        );
        assert!(
            !mode.contains(TermMode::APP_CURSOR),
            "application cursor keys cleared"
        );
        assert!(mode.contains(TermMode::LINE_WRAP), "autowrap back on");
        assert!(mode.contains(TermMode::SHOW_CURSOR), "cursor shown again");

        // The widget's report gate reads the mode back from the state: a
        // pointer move must not synthesize a report any more.
        let view = TerminalView::<()>::new(Arc::new(Mutex::new(term)));
        let mut ws = TerminalWidgetState::default();
        let ev = iced::Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(40.0, 40.0),
        });
        let action = view.handle_mouse_report(
            &mut ws,
            &ev,
            bounds(),
            mouse::Cursor::Available(Point::new(40.0, 40.0)),
            mode,
            80,
            24,
        );
        assert!(action.is_none(), "no SGR report after the session reset");
    }

    /// The scrolling region a dead full-screen app left behind would pin
    /// the new shell's output inside its band, and DECSTBM homes the
    /// cursor, which is why the reset wraps it in DECSC/DECRC: the
    /// region goes back to the whole screen and the cursor stays where
    /// the session left it.
    #[test]
    fn session_reset_restores_the_scrolling_region_without_moving_the_cursor() {
        use alacritty_terminal::index::{Column, Line};

        let mut term = TerminalState::new_no_pty(80, 24).unwrap();
        // A band over the top 5 lines, cursor parked inside the pane.
        term.process(b"\x1b[1;5r\x1b[10;3H");
        let before = term.backend.term.grid().cursor.point;
        assert_eq!(before, alacritty_terminal::index::Point::new(Line(9), Column(2)));

        term.process(crate::SESSION_MODE_RESET);
        assert_eq!(
            term.backend.term.grid().cursor.point, before,
            "the region reset must not home the cursor"
        );

        // One line per row, no trailing newline: with the band still
        // armed the text would scroll inside rows 1-5 and the bottom of
        // the screen would stay empty.
        term.process(b"\x1b[H");
        for i in 0..24 {
            term.process(format!("L{i}").as_bytes());
            if i < 23 {
                term.process(b"\r\n");
            }
        }
        let last = (0..3)
            .map(|c| term.backend.term.grid()[Line(23)][Column(c)].c)
            .collect::<String>();
        assert_eq!(last, "L23", "output must reach the bottom row again");
    }

    /// A connection killed inside tmux / vim leaves the pane on the
    /// alternate screen. `LEAVE_ALT_SCREEN` puts it back on the real
    /// buffer (with its scrollback) exactly as the app's own clean exit
    /// would have, and is a no-op on a pane that never entered.
    #[test]
    fn leave_alt_screen_restores_the_primary_buffer() {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::TermMode;

        let mut term = TerminalState::new_no_pty(80, 24).unwrap();
        term.process(b"shell output");
        // A full-screen app takes over and dies mid-frame.
        term.process(b"\x1b[?1049h\x1b[Happ frame");
        assert!(term.backend.term.mode().contains(TermMode::ALT_SCREEN));

        term.process(crate::LEAVE_ALT_SCREEN);

        assert!(
            !term.backend.term.mode().contains(TermMode::ALT_SCREEN),
            "back on the primary buffer"
        );
        let row0 = (0..12)
            .map(|c| term.backend.term.grid()[Line(0)][Column(c)].c)
            .collect::<String>();
        assert_eq!(row0, "shell output", "the real buffer is back");

        // Idempotent: a pane that never entered stays put.
        term.process(crate::LEAVE_ALT_SCREEN);
        assert!(!term.backend.term.mode().contains(TermMode::ALT_SCREEN));
        let row0 = (0..12)
            .map(|c| term.backend.term.grid()[Line(0)][Column(c)].c)
            .collect::<String>();
        assert_eq!(row0, "shell output");
    }

    // ── The viewport offset is the grid's display_offset ──

    /// The text on the viewport's top row, straight from the grid.
    fn top_row(state: &TerminalState) -> String {
        use alacritty_terminal::index::{Column, Line};
        let grid = state.backend.term.grid();
        let line = Line(-state.viewport_offset());
        (0..8)
            .map(|c| grid[line][Column(c)].c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Feed `L<i>` lines, one per row, so a row's text says which one it is.
    fn numbered(state: &mut TerminalState, range: std::ops::Range<usize>) {
        for i in range {
            state.process(format!("L{i}\r\n").as_bytes());
        }
    }

    /// A viewport scrolled up keeps showing the same rows while output
    /// arrives: the grid raises `display_offset` as lines scroll into
    /// history, and the widget reads that back instead of keeping a count
    /// of its own.
    #[test]
    fn scrolled_viewport_holds_its_rows_through_output() {
        let (view, mut ws) = scrolled_view(200);
        numbered(&mut view.state.lock().unwrap(), 0..30);
        let cursor = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let notch = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
        });
        view.on_event(&mut ws, &notch, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 3, "one notch, three lines up");
        let before = top_row(&view.state.lock().unwrap());

        {
            let mut s = view.state.lock().unwrap();
            numbered(&mut s, 30..35);
            assert_eq!(s.viewport_offset(), 8, "five lines of output raise the offset by five");
            assert_eq!(top_row(&s), before, "the rows being read stay on screen");
        }
        // The next gesture builds on the grid's answer, not on a stale count.
        view.on_event(&mut ws, &notch, bounds(), cursor);
        assert_eq!(ws.scroll_offset.get(), 11);
    }

    /// A full scrollback keeps holding: the history stops growing at the
    /// cap, but lines still rotate through it, and the offset follows the
    /// rotation until the rows themselves leave the buffer.
    #[test]
    fn a_full_scrollback_still_holds_the_rows() {
        use alacritty_terminal::grid::Dimensions;
        let mut s = TerminalState::new_no_pty_with_scrollback(80, 3, 5).unwrap();
        numbered(&mut s, 0..8);
        assert_eq!(s.backend.term.grid().history_size(), 5, "at the cap");
        s.scroll_viewport_by(2);
        let before = top_row(&s);

        numbered(&mut s, 8..11);
        assert_eq!(s.backend.term.grid().history_size(), 5, "the cap does not move");
        assert_eq!(s.viewport_offset(), 5, "three more lines, three more up");
        assert_eq!(top_row(&s), before, "the rows being read stay on screen");

        // Once they fall off the top there is nothing older to show: the
        // offset stays at the cap and the view follows the oldest line
        // (fourteen lines plus the cursor row, eight kept: L7 on top).
        numbered(&mut s, 11..14);
        assert_eq!(s.viewport_offset(), 5);
        assert_eq!(top_row(&s), "L7");
    }

    /// Clearing the scrollback lands on the live edge (there is nothing
    /// above it to show), and output after that follows the edge until
    /// the user scrolls again.
    #[test]
    fn clear_scrollback_lands_on_the_live_edge() {
        let mut s = TerminalState::new_no_pty_with_scrollback(80, 3, 100).unwrap();
        numbered(&mut s, 0..10);
        s.scroll_viewport_by(4);
        assert_eq!(s.viewport_offset(), 4);
        s.clear_scrollback();
        assert_eq!(s.viewport_offset(), 0);
        numbered(&mut s, 10..13);
        assert_eq!(s.viewport_offset(), 0, "at the live edge, output is followed");
    }

    /// A resize keeps the same rows in view: a shorter window pushes lines
    /// into history and the offset rises with them, a taller one pulls
    /// lines back and the offset falls, down to the live edge.
    #[test]
    fn a_resize_keeps_the_rows_in_view() {
        let mut s = TerminalState::new_no_pty_with_scrollback(80, 5, 100).unwrap();
        numbered(&mut s, 0..10);
        s.scroll_viewport_by(3);
        let before = top_row(&s);
        s.resize(80, 3);
        assert_eq!(s.viewport_offset(), 5, "two rows pushed into history");
        assert_eq!(top_row(&s), before);
        s.resize(80, 8);
        assert_eq!(s.viewport_offset(), 0, "five rows pulled back onto the screen");
    }

    /// The alternate screen is a grid of its own with no history: it reads
    /// 0 while an app owns it, a scroll there goes nowhere, and the primary
    /// buffer's position is back untouched on the way out.
    #[test]
    fn alt_screen_round_trip_keeps_the_primary_offset() {
        let mut s = TerminalState::new_no_pty_with_scrollback(80, 3, 100).unwrap();
        numbered(&mut s, 0..10);
        s.scroll_viewport_by(3);
        s.process(b"\x1b[?1049h\x1b[Happ frame");
        assert_eq!(s.viewport_offset(), 0);
        assert_eq!(s.scroll_viewport_by(2), 0, "nothing to scroll into");
        s.process(b"\x1b[?1049l");
        assert_eq!(s.viewport_offset(), 3);
    }

    /// A program scrolling a region that does not start at the top row (a
    /// fixed header under DECSTBM) bumps the grid's raw offset without
    /// adding a line to history. What the widget reads never passes the
    /// top, the visible-screen export stays inside the buffer, and the
    /// first gesture after it measures from the row on screen rather than
    /// from the raw excess.
    #[test]
    fn a_region_scroll_never_reads_past_the_top() {
        use alacritty_terminal::grid::Dimensions;
        let mut s = TerminalState::new_no_pty_with_scrollback(80, 4, 100).unwrap();
        numbered(&mut s, 0..10);
        s.scroll_viewport_by(3);
        s.process(b"\x1b[2;4r\x1b[4;1H");
        for i in 0..10 {
            s.process(format!("R{i}\n").as_bytes());
        }
        let grid = s.backend.term.grid();
        let history = grid.history_size() as i32;
        assert!(grid.display_offset() as i32 > history, "the raw offset passed the top");
        assert_eq!(s.viewport_offset(), history, "read at the top, never past it");
        assert_eq!(top_row(&s), "L0", "the oldest row the grid holds");
        assert_eq!(s.visible_text(), "L0\nL1\nL2\nL3");
        assert_eq!(s.scroll_viewport_by(-3), history - 3, "a wheel-down moves from the row on screen");
    }

    /// A queued absolute target resolves against the grid as it is when
    /// applied: past the top lands on the oldest screen, 0 (or less) is
    /// the live edge.
    #[test]
    fn an_absolute_target_is_clamped_to_the_grid() {
        let mut s = TerminalState::new_no_pty_with_scrollback(80, 3, 100).unwrap();
        numbered(&mut s, 0..10);
        assert_eq!(s.scroll_viewport_to(i32::MAX), 8, "eleven rows, three on screen");
        assert_eq!(s.scroll_viewport_to(0), 0);
        assert_eq!(s.scroll_viewport_to(-4), 0);
    }
