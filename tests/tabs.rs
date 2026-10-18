use egui::{pos2, vec2, Color32, Rect};
use redline::{
    state::{AppState, CanvasState, MarqueeState, SelKind, Selection},
    types::{Annotation, ShapeData},
};

fn make_app_with_n_tabs(n: usize) -> AppState {
    let mut state = AppState::default();
    state.tab_label = "tab0".into();
    state.canvas.canvas_size = vec2(100.0, 100.0);
    state.canvas.annotations.push(Annotation::new(
        ShapeData::plain_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 1.0))),
        Color32::RED,
        1.0,
    ));
    for i in 1..n {
        state.push_current_as_tab_and_reset(format!("tab{i}"));
        state.canvas.canvas_size = vec2(100.0, 100.0);
        state.canvas.annotations.push(Annotation::new(
            ShapeData::plain_rect(Rect::from_min_size(pos2(i as f32, 0.0), vec2(1.0, 1.0))),
            Color32::RED,
            1.0,
        ));
    }
    state
}

fn tab_marker_x(canvas: &CanvasState) -> f32 {
    let shape = &canvas.annotations[0].shape;
    let ShapeData::Rect { rect, .. } = shape else {
        panic!("expected Rect marker")
    };
    rect.min.x
}

#[test]
fn switch_to_tab_preserves_label_canvas_pairing() {
    let mut state = make_app_with_n_tabs(4);
    assert_eq!(state.tab_label, "tab3");
    assert_eq!(tab_marker_x(&state.canvas), 3.0);

    for n in 0..4 {
        state.switch_to_tab(n);
        assert_eq!(
            state.tab_label,
            format!("tab{n}"),
            "label mismatch at n={n}"
        );
        assert_eq!(
            tab_marker_x(&state.canvas),
            n as f32,
            "canvas marker mismatch at n={n}"
        );
    }
}

#[test]
fn reorder_tab_three_cycle_preserves_pairing() {
    let mut state = make_app_with_n_tabs(3);
    state.switch_to_tab(0);
    state.reorder_tab(0, 2);
    assert_eq!(state.active_tab_pos, 2);
    assert_eq!(state.tab_label, "tab0");
    assert_eq!(tab_marker_x(&state.canvas), 0.0);
    assert_eq!(state.tab_label_at(0), "tab1");
    assert_eq!(state.tab_label_at(1), "tab2");
    assert_eq!(state.tab_label_at(2), "tab0");
}

#[test]
fn close_active_tab_promotes_neighbor_with_correct_canvas() {
    let mut state = make_app_with_n_tabs(3);
    state.close_tab(state.active_tab_pos);
    assert_eq!(state.tab_label, "tab1");
    assert_eq!(tab_marker_x(&state.canvas), 1.0);
}

#[test]
fn selection_follows_tab_on_switch() {
    let mut state = make_app_with_n_tabs(2);
    state.canvas.selection = Some(Selection {
        kind: SelKind::Rect,
        rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)),
        inverted: false,
    });
    state.switch_to_tab(0);
    assert!(
        state.canvas.selection.is_none(),
        "tab0 should have no selection"
    );
    state.switch_to_tab(1);
    assert!(
        state.canvas.selection.is_some(),
        "tab1's selection should return"
    );
}

#[test]
fn pdf_page_follows_tab_on_switch() {
    let mut state = make_app_with_n_tabs(2);
    state.canvas.pdf_page = 3;
    state.canvas.pdf_pages = 10;
    state.switch_to_tab(0);
    assert_eq!(state.canvas.pdf_page, 0);
    assert_eq!(state.canvas.pdf_pages, 0);
    state.switch_to_tab(1);
    assert_eq!(state.canvas.pdf_page, 3);
    assert_eq!(state.canvas.pdf_pages, 10);
}

#[test]
fn marquee_state_follows_tab_on_switch() {
    let mut state = make_app_with_n_tabs(2);
    state.canvas.marquee_state = MarqueeState::Drawing {
        start: pos2(1.0, 2.0),
    };
    state.switch_to_tab(0);
    assert!(matches!(state.canvas.marquee_state, MarqueeState::Idle));
    state.switch_to_tab(1);
    assert!(matches!(
        state.canvas.marquee_state,
        MarqueeState::Drawing { .. }
    ));
}

#[test]
fn file_tabs_interactive_only_for_file_modes() {
    use redline::state::AppMode;
    assert!(AppMode::Annotate.file_tabs_interactive());
    assert!(AppMode::NativeInpaint.file_tabs_interactive());
    assert!(AppMode::SvgEditor.file_tabs_interactive());
    assert!(!AppMode::CanvasSort.file_tabs_interactive());
    assert!(!AppMode::LamaCleaner.file_tabs_interactive());
}

#[test]
fn tab_bar_shows_when_multiple_tabs_regardless_of_mode() {
    use redline::state::AppMode;
    let mut state = make_app_with_n_tabs(3);
    state.app_mode = AppMode::CanvasSort;
    assert!(
        state.should_show_tab_bar(),
        "tabs must remain visible in folder modes"
    );
    state.app_mode = AppMode::Annotate;
    assert!(state.should_show_tab_bar());
}

#[test]
fn tab_bar_hidden_for_empty_single_canvas() {
    let state = AppState::default();
    assert!(
        !state.should_show_tab_bar(),
        "no texture and no inactive tabs → hidden"
    );
}

#[test]
fn switching_app_mode_preserves_tabs_and_active_canvas() {
    use redline::state::AppMode;
    let mut state = make_app_with_n_tabs(3);
    let active_marker = tab_marker_x(&state.canvas);
    let count = state.tab_count();
    state.app_mode = AppMode::LamaCleaner;
    assert_eq!(
        state.tab_count(),
        count,
        "tab count must survive mode switch"
    );
    assert_eq!(
        tab_marker_x(&state.canvas),
        active_marker,
        "active canvas must survive"
    );
    state.app_mode = AppMode::Annotate;
    assert_eq!(state.tab_count(), count);
    assert_eq!(tab_marker_x(&state.canvas), active_marker);
}
