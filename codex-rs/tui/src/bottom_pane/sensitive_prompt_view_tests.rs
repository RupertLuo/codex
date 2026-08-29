use super::*;
use crate::bottom_pane::BottomPaneView;
use crate::bottom_pane::ViewCompletion;
use crate::render::renderable::Renderable;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::sync::mpsc::Receiver;
use std::time::Duration;
use std::time::Instant;

#[test]
fn typed_secret_is_masked_and_submitted_by_ownership() {
    let (mut view, submitted_rx) = sensitive_prompt_view();
    let now = Instant::now();
    for (index, character) in "seeded-secret-marker".chars().enumerate() {
        view.handle_key_event_at(
            KeyEvent::from(KeyCode::Char(character)),
            now + Duration::from_millis(index as u64 * 20),
        );
    }

    let rendered = render_view(&view);
    assert!(!rendered.contains("seeded-secret-marker"));
    assert!(rendered.contains("••••"));

    view.handle_key_event_at(
        KeyEvent::from(KeyCode::Enter),
        now + Duration::from_millis(500),
    );
    let submitted = submitted_rx.recv().expect("secret should be submitted");
    assert_eq!(submitted.expose_secret(), "seeded-secret-marker");
    assert!(view.is_complete());
}

#[test]
fn pasted_secret_is_masked_and_submitted_by_ownership() {
    let (mut view, submitted_rx) = sensitive_prompt_view();
    assert!(view.handle_paste("seeded-secret-marker".to_string()));

    let rendered = render_view(&view);
    assert!(!rendered.contains("seeded-secret-marker"));
    assert!(rendered.contains("••••"));

    view.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let submitted = submitted_rx.recv().expect("secret should be submitted");
    assert_eq!(submitted.expose_secret(), "seeded-secret-marker");
    assert!(view.is_complete());
}

#[test]
fn escape_cancels_without_submitting() {
    let (mut view, submitted_rx) = sensitive_prompt_view();
    assert!(view.handle_paste("seeded-secret-marker".to_string()));

    view.handle_key_event(KeyEvent::from(KeyCode::Esc));

    assert!(submitted_rx.try_recv().is_err());
    assert!(view.is_complete());
    assert_eq!(view.completion(), Some(ViewCompletion::Cancelled));
}

fn sensitive_prompt_view() -> (SensitivePromptView, Receiver<SensitiveInput>) {
    let (submitted, submitted_rx) = std::sync::mpsc::channel();
    let view = SensitivePromptView::new(
        "Enter credential".to_string(),
        "Paste a credential and press Enter".to_string(),
        Some("Test provider".to_string()),
        Box::new(move |value| {
            submitted.send(value).expect("send submitted secret");
        }),
    );
    (view, submitted_rx)
}

fn render_view(view: &SensitivePromptView) -> String {
    let area = Rect::new(0, 0, 80, view.desired_height(80));
    let mut buffer = Buffer::empty(area);
    view.render(area, &mut buffer);
    buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}
