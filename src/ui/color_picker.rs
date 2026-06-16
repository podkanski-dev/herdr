use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use super::widgets::{
    action_button_row_rects, centered_popup_rect, panel_contrast_fg, render_action_button,
    render_modal_header, render_modal_shell, ActionButtonSpec,
};
use crate::app::{state::ColorPickerSelection, AppState};

const MODAL_WIDTH: u16 = 44;
const MODAL_HEIGHT: u16 = 12;

/// Clickable regions of the picker modal, for render + mouse hit-testing.
pub(crate) struct ColorPickerLayout {
    pub swatch_cells: Vec<Rect>,
    pub none_cell: Rect,
    pub hex_input: Rect,
    pub save: Rect,
    pub cancel: Rect,
}

/// Inner rect of the modal shell, computed WITHOUT drawing (for hit-testing).
pub(crate) fn picker_inner_rect(area: Rect) -> Option<Rect> {
    centered_popup_rect(area, MODAL_WIDTH, MODAL_HEIGHT).map(|popup| {
        Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        )
    })
}

pub(crate) fn color_picker_layout(area: Rect) -> Option<ColorPickerLayout> {
    let inner = picker_inner_rect(area)?;
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // spacer
        Constraint::Length(1), // swatch row
        Constraint::Length(1), // none row
        Constraint::Length(1), // spacer
        Constraint::Length(1), // hex input
        Constraint::Min(0),    // buttons area
    ])
    .areas::<7>(inner);

    let swatch_cells = (0..7)
        .map(|i| Rect::new(rows[2].x + i as u16 * 5, rows[2].y, 4, 1))
        .collect();
    let none_cell = Rect::new(rows[3].x, rows[3].y, 8, 1);

    let buttons = action_button_row_rects(
        inner,
        &[
            ActionButtonSpec {
                hint: Some("↵"),
                label: "save",
            },
            ActionButtonSpec {
                hint: Some("esc"),
                label: "cancel",
            },
        ],
        2,
        inner.height.saturating_sub(1),
    );

    Some(ColorPickerLayout {
        swatch_cells,
        none_cell,
        hex_input: Rect::new(rows[5].x, rows[5].y, rows[5].width, 1),
        save: buttons[0],
        cancel: buttons[1],
    })
}

pub(super) fn render_color_picker(app: &AppState, frame: &mut Frame, area: Rect) {
    super::dim_background(frame, area);
    let Some(picker) = app.color_picker.as_ref() else {
        return;
    };
    let Some(inner) = render_modal_shell(frame, area, MODAL_WIDTH, MODAL_HEIGHT, &app.palette)
    else {
        return;
    };
    let Some(layout) = color_picker_layout(area) else {
        return;
    };

    render_modal_header(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        "workspace color",
        &app.palette,
    );

    for (idx, (cell, (_name, color))) in layout.swatch_cells.iter().zip(&picker.swatches).enumerate() {
        let selected = picker.selected == ColorPickerSelection::Swatch(idx);
        let block = if selected { "[██]" } else { " ██ " };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                block,
                Style::default().fg(*color),
            ))),
            *cell,
        );
    }

    let none_selected = picker.selected == ColorPickerSelection::Clear;
    let none_label = if none_selected { "[none]" } else { " none " };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            none_label,
            Style::default().fg(app.palette.subtext0),
        ))),
        layout.none_cell,
    );

    let custom = picker.selected == ColorPickerSelection::Custom;
    let caret = if custom { "█" } else { "" };
    frame.render_widget(Clear, layout.hex_input);
    frame.render_widget(
        Paragraph::new(format!(" hex: {}{}", picker.hex_input, caret))
            .style(Style::default().fg(app.palette.text).bg(app.palette.surface0)),
        layout.hex_input,
    );

    if let Some(error) = &picker.error {
        let err_rect = Rect::new(inner.x, layout.save.y.saturating_sub(1), inner.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(app.palette.red),
            ))),
            err_rect,
        );
    }

    render_action_button(
        frame,
        layout.save,
        Some("↵"),
        "save",
        Style::default()
            .fg(panel_contrast_fg(&app.palette))
            .bg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    render_action_button(
        frame,
        layout.cancel,
        Some("esc"),
        "cancel",
        Style::default()
            .fg(app.palette.text)
            .bg(app.palette.surface0)
            .add_modifier(Modifier::BOLD),
    );
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::color_picker_layout;

    #[test]
    fn layout_has_seven_distinct_swatch_cells() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = color_picker_layout(area).expect("layout fits in a large area");
        assert_eq!(layout.swatch_cells.len(), 7);
        // swatch cells are on the same row, strictly increasing x, non-overlapping
        for w in layout.swatch_cells.windows(2) {
            assert_eq!(w[0].y, w[1].y);
            assert!(
                w[1].x >= w[0].x + w[0].width,
                "swatch cells must not overlap"
            );
        }
        // save/cancel/none/hex are non-empty
        assert!(layout.save.width > 0 && layout.cancel.width > 0);
        assert!(layout.none_cell.width > 0 && layout.hex_input.width > 0);
    }
}
