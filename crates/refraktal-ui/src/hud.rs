// SPDX-License-Identifier: GPL-3.0-or-later
//! All text on screen: tempo, track names, status line and the help overlay.

use crate::layout::{HELP_ROWS, Layout};
use crate::renderer::FrameState;
use crate::text::{Align, TextRenderer, Weight};

const BRIGHT: [f32; 4] = [0.92, 0.90, 1.0, 1.0];
const DIM: [f32; 4] = [0.62, 0.58, 0.78, 1.0];
const FAINT: [f32; 4] = [0.45, 0.42, 0.60, 1.0];

/// Gesture, then what it does, for touch screens.
const TOUCH_HELP: [(&str, &str); 9] = [
    ("Tap a cell", "Turn a step on or off"),
    ("Tap the play button", "Play or stop"),
    ("Tap  −  or  +", "Tempo down or up by 5 BPM"),
    ("Tap a dot", "Select a track and hear it"),
    ("Tap the selected dot", "Switch to the next built-in sound"),
    ("Tap + under the tracks", "Add a track"),
    ("Hold a dot", "Remove the track"),
    ("Tap ?", "Show or hide this help"),
    ("", "Your beat is saved when you leave the app"),
];

/// Shortcut, then what it does. Keep in sync with the README.
const HELP: [(&str, &str); HELP_ROWS] = [
    ("Click a cell", "Turn a step on or off"),
    ("Space", "Play or stop"),
    ("↑  ↓", "Tempo up or down by 5 BPM"),
    ("1 to 8, or click a dot", "Select a track and hear it"),
    ("Click the selected dot", "Switch to the next built-in sound"),
    ("+", "Add a track"),
    ("Delete", "Remove the selected track"),
    ("Drop an audio file", "Play it on the selected track"),
    ("Right-click a dot", "Remove its sample, or the track"),
    ("Ctrl+S,  Ctrl+Shift+S", "Save, save as"),
    ("Ctrl+O,  Ctrl+N", "Open a project, start a new one"),
    ("F1, Esc or ?", "Close this help"),
];

pub(crate) fn queue(text: &mut TextRenderer, layout: &Layout, frame: &FrameState) {
    // The overlay dims everything behind it; text is drawn last, so hide the
    // rest of the HUD instead of letting it shine through.
    if frame.help_visible {
        queue_help(text, layout, frame);
        return;
    }
    let s = layout.scale;

    // Label for the help button; its circle is drawn by the shader.
    let (hx, hy, _) = layout.help_button;
    text.queue("?", hx, hy + 5.5 * s, 16.0 * s, Weight::Bold, DIM, Align::Center);

    // Tempo readout in the transport pill.
    let (bx, by) = layout.bpm_anchor;
    let unit_w = text.measure(" BPM", 12.0 * s, Weight::Regular);
    text.queue(" BPM", bx, by, 12.0 * s, Weight::Regular, DIM, Align::Right);
    text.queue(&format!("{:.0}", frame.bpm), bx - unit_w, by, 21.0 * s, Weight::Bold, BRIGHT, Align::Right);

    // Track names.
    for (track, label) in frame.track_labels.iter().enumerate().take(layout.track_count) {
        let color = if track == frame.selected_track { BRIGHT } else { DIM };
        let label = truncate(label, 10);
        text.queue(&label, layout.name_x, layout.row_baseline(track), 14.0 * s, Weight::Regular, color, Align::Left);
    }

    // Status line: a recent message, otherwise a pointer to the help.
    let (sx, sy) = layout.status_anchor;
    match &frame.status {
        Some((message, alpha)) => {
            let color = [BRIGHT[0], BRIGHT[1], BRIGHT[2], alpha.clamp(0.0, 1.0)];
            text.queue(message, sx, sy, 13.0 * s, Weight::Regular, color, Align::Center);
        }
        None if !frame.touch => {
            text.queue("Press F1 for help", sx, sy, 13.0 * s, Weight::Regular, FAINT, Align::Center);
        }
        None => {}
    }
}

fn queue_help(text: &mut TextRenderer, layout: &Layout, frame: &FrameState) {
    let s = layout.scale;
    let panel = layout.help;
    let left = panel.x + if layout.compact { 28.0 } else { 44.0 } * s;
    let middle = panel.x + panel.w * 0.43;
    let (title_y, first_row) = if layout.compact { (40.0, 72.0) } else { (58.0, 108.0) };

    let (title, rows): (&str, &[(&str, &str)]) = if frame.touch {
        let rows = if frame.autosave { &TOUCH_HELP[..] } else { &TOUCH_HELP[..TOUCH_HELP.len() - 1] };
        ("How to play", rows)
    } else {
        ("Keyboard and mouse", &HELP[..])
    };
    text.queue(title, left, panel.y + title_y * s, 24.0 * s, Weight::Bold, BRIGHT, Align::Left);
    for (i, (keys, action)) in rows.iter().enumerate() {
        let y = panel.y + first_row * s + i as f32 * layout.help_row;
        text.queue(keys, left, y, 15.0 * s, Weight::Bold, BRIGHT, Align::Left);
        text.queue(action, middle, y, 15.0 * s, Weight::Regular, DIM, Align::Left);
    }
    let footer = format!(
        "Refraktal {}. Free software under the GPL, version 3 or later. Works fully offline.",
        env!("CARGO_PKG_VERSION")
    );
    let footer_y = panel.y + panel.h - if layout.compact { 18.0 } else { 30.0 } * s;
    text.queue(&footer, left, footer_y, 12.0 * s, Weight::Regular, FAINT, Align::Left);
}

/// Shorten long names, keeping them recognizable.
fn truncate(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        label.to_owned()
    } else {
        let mut short: String = label.chars().take(max_chars - 1).collect();
        short.push('…');
        short
    }
}
