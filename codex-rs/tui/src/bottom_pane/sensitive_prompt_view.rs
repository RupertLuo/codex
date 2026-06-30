use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use std::cell::RefCell;
use std::time::Instant;
use zeroize::Zeroize;

use crate::key_hint::has_ctrl_or_alt;
use crate::model_runtime::SensitiveInput;
use crate::render::renderable::Renderable;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::bottom_pane_view::ViewCompletion;
use super::paste_burst::PasteBurst;
use super::popup_consts::standard_popup_hint_line;
use super::textarea::TextArea;
use super::textarea::TextAreaState;

pub(crate) type SensitivePromptSubmitted = Box<dyn Fn(SensitiveInput) + Send + Sync>;
pub(crate) type SensitivePromptCancelled = Box<dyn Fn() + Send + Sync>;

pub(crate) struct SensitivePromptView {
    title: String,
    placeholder: String,
    context_label: Option<String>,
    on_submit: SensitivePromptSubmitted,
    on_cancel: Option<SensitivePromptCancelled>,
    textarea: TextArea,
    textarea_state: RefCell<TextAreaState>,
    paste_burst: PasteBurst,
    completion: Option<ViewCompletion>,
}

impl SensitivePromptView {
    pub(crate) fn new(
        title: String,
        placeholder: String,
        context_label: Option<String>,
        on_submit: SensitivePromptSubmitted,
    ) -> Self {
        Self {
            title,
            placeholder,
            context_label,
            on_submit,
            on_cancel: None,
            textarea: TextArea::new(),
            textarea_state: RefCell::new(TextAreaState::default()),
            paste_burst: PasteBurst::default(),
            completion: None,
        }
    }

    pub(crate) fn with_cancel(mut self, on_cancel: SensitivePromptCancelled) -> Self {
        self.on_cancel = Some(on_cancel);
        self
    }

    fn handle_key_event_at(&mut self, key_event: KeyEvent, now: Instant) {
        match key_event {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.on_ctrl_c();
            }
            KeyEvent {
                code: KeyCode::Char('k' | 'y'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) => {
                self.paste_burst.clear_after_explicit_paste();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers,
                ..
            } => {
                if self.paste_burst.direct_insert_newline_should_insert(now) {
                    self.paste_burst.extend_window(now);
                    self.textarea.insert_str("\n");
                    return;
                }
                if modifiers == KeyModifiers::NONE {
                    let value = SensitiveInput::new(self.textarea.take_sensitive_text());
                    if !value.expose_secret().trim().is_empty() {
                        (self.on_submit)(value);
                        self.completion = Some(ViewCompletion::Accepted);
                    }
                } else {
                    self.textarea.input(key_event);
                }
            }
            KeyEvent {
                code: KeyCode::Char(_),
                modifiers,
                ..
            } if !has_ctrl_or_alt(modifiers) && self.textarea.allows_paste_burst() => {
                let paste_like_burst = self.paste_burst.on_plain_char_no_hold(now).is_some();
                self.textarea.input(key_event);
                if paste_like_burst {
                    self.paste_burst.extend_window(now);
                }
            }
            KeyEvent {
                code: KeyCode::Tab,
                modifiers,
                ..
            } if !has_ctrl_or_alt(modifiers) && self.textarea.allows_paste_burst() => {
                let in_paste_burst = self.paste_burst.direct_insert_newline_should_insert(now);
                self.textarea.input(key_event);
                if in_paste_burst {
                    self.paste_burst.extend_window(now);
                }
            }
            other => {
                self.textarea.input(other);
                self.paste_burst.clear_after_explicit_paste();
            }
        }
    }

    fn input_height(&self, width: u16) -> u16 {
        let usable_width = width.saturating_sub(2);
        let text_height = self.textarea.desired_height(usable_width).clamp(1, 8);
        text_height.saturating_add(1).min(9)
    }
}

impl BottomPaneView for SensitivePromptView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        self.handle_key_event_at(key_event, Instant::now());
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.completion = Some(ViewCompletion::Cancelled);
        if let Some(on_cancel) = self.on_cancel.take() {
            on_cancel();
        }
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.completion.is_some()
    }

    fn completion(&self) -> Option<ViewCompletion> {
        self.completion
    }

    fn handle_paste(&mut self, mut pasted: String) -> bool {
        if pasted.is_empty() {
            return false;
        }
        self.textarea.insert_str(&pasted);
        pasted.zeroize();
        self.paste_burst.clear_after_explicit_paste();
        true
    }
}

impl Renderable for SensitivePromptView {
    fn desired_height(&self, width: u16) -> u16 {
        let extra_top: u16 = if self.context_label.is_some() { 1 } else { 0 };
        1u16 + extra_top + self.input_height(width) + 3u16
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let input_height = self.input_height(area.width);
        let title_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        Paragraph::new(Line::from(vec![gutter(), self.title.clone().bold()]))
            .render(title_area, buf);

        let mut input_y = area.y.saturating_add(1);
        if let Some(context_label) = &self.context_label {
            let context_area = Rect {
                x: area.x,
                y: input_y,
                width: area.width,
                height: 1,
            };
            Paragraph::new(Line::from(vec![gutter(), context_label.clone().cyan()]))
                .render(context_area, buf);
            input_y = input_y.saturating_add(1);
        }

        let input_area = Rect {
            x: area.x,
            y: input_y,
            width: area.width,
            height: input_height,
        };
        if input_area.width >= 2 {
            for row in 0..input_area.height {
                Paragraph::new(Line::from(vec![gutter()])).render(
                    Rect {
                        x: input_area.x,
                        y: input_area.y.saturating_add(row),
                        width: 2,
                        height: 1,
                    },
                    buf,
                );
            }

            let text_area_height = input_area.height.saturating_sub(1);
            if text_area_height > 0 {
                if input_area.width > 2 {
                    Clear.render(
                        Rect {
                            x: input_area.x.saturating_add(2),
                            y: input_area.y,
                            width: input_area.width.saturating_sub(2),
                            height: 1,
                        },
                        buf,
                    );
                }
                let textarea_rect = Rect {
                    x: input_area.x.saturating_add(2),
                    y: input_area.y.saturating_add(1),
                    width: input_area.width.saturating_sub(2),
                    height: text_area_height,
                };
                let mut state = self.textarea_state.borrow_mut();
                self.textarea
                    .render_ref_masked(textarea_rect, buf, &mut state, '•');
                if self.textarea.text().is_empty() {
                    Paragraph::new(Line::from(self.placeholder.clone().dim()))
                        .render(textarea_rect, buf);
                }
            }
        }

        let hint_blank_y = input_area.y.saturating_add(input_height);
        if hint_blank_y < area.y.saturating_add(area.height) {
            Clear.render(
                Rect {
                    x: area.x,
                    y: hint_blank_y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }

        let hint_y = hint_blank_y.saturating_add(1);
        if hint_y < area.y.saturating_add(area.height) {
            Paragraph::new(standard_popup_hint_line()).render(
                Rect {
                    x: area.x,
                    y: hint_y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if area.height < 2 || area.width <= 2 {
            return None;
        }
        let text_area_height = self.input_height(area.width).saturating_sub(1);
        if text_area_height == 0 {
            return None;
        }
        let extra_offset: u16 = if self.context_label.is_some() { 1 } else { 0 };
        let textarea_rect = Rect {
            x: area.x.saturating_add(2),
            y: area.y.saturating_add(1u16 + extra_offset).saturating_add(1),
            width: area.width.saturating_sub(2),
            height: text_area_height,
        };
        let state = *self.textarea_state.borrow();
        self.textarea.cursor_pos_with_state(textarea_rect, state)
    }
}

impl Drop for SensitivePromptView {
    fn drop(&mut self) {
        drop(SensitiveInput::new(self.textarea.take_sensitive_text()));
    }
}

fn gutter() -> Span<'static> {
    "▌ ".cyan()
}

#[cfg(test)]
#[path = "sensitive_prompt_view_tests.rs"]
mod tests;
