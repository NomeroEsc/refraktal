// SPDX-License-Identifier: GPL-3.0-or-later
//! All text on screen: tempo, track names, status line and the help overlay.

use crate::layout::{Chip, HELP_ROWS, Layout, TOUCH_HELP_ROWS};
use crate::renderer::FrameState;
use crate::text::{Align, TextRenderer, Weight};

const BRIGHT: [f32; 4] = [0.92, 0.90, 1.0, 1.0];
const DIM: [f32; 4] = [0.62, 0.58, 0.78, 1.0];
const FAINT: [f32; 4] = [0.45, 0.42, 0.60, 1.0];

/// Gesture, then what it does, for touch screens.
const TOUCH_HELP: [(&str, &str); TOUCH_HELP_ROWS] = [
    ("Tap a cell", "Turn a step on or off"),
    ("Tap the play button", "Play or stop"),
    ("Tap  −  or  +", "Tempo down or up by 5 BPM"),
    ("Tap a sound", "Add a track with that sound"),
    ("Tap a dot", "Select a track and hear it"),
    ("Tap the selected dot", "Switch to the next built-in sound"),
    ("Hold a dot", "Remove the track"),
    ("Tap a pattern, hold it", "Switch to it, delete it"),
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
    ("Click a sound", "Add a track with that sound"),
    ("Click a pattern, right-click", "Switch to it, delete it"),
    ("Delete", "Remove the selected track"),
    ("Drop an audio file", "Play it on the selected track"),
    ("Right-click a dot", "Remove its sample, or the track"),
    ("Ctrl+S,  Ctrl+Shift+S", "Save, save as"),
    ("Ctrl+O,  Ctrl+N", "Open a project, start a new one"),
    ("Ctrl+E", "Export the pattern to WAV"),
    ("F1, Esc or ?", "Close this help"),
];

pub(crate) fn queue(text: &mut TextRenderer, layout: &Layout, frame: &FrameState) {
    // The overlay dims everything behind it; text is drawn last, so hide the
    // rest of the HUD instead of letting it shine through.
    if frame.notice_visible {
        queue_notice(text, layout, frame);
        return;
    }
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

    // Track names, or a hint in an empty pattern.
    for (track, label) in frame.track_labels.iter().enumerate().take(layout.track_count) {
        let color = if track == frame.selected_track { BRIGHT } else { DIM };
        let label = truncate(label, 10);
        text.queue(&label, layout.name_x, layout.row_baseline(track), 14.0 * s, Weight::Regular, color, Align::Left);
    }
    if layout.track_count == 0 {
        let panel = layout.sequencer;
        let (x, y) = (panel.x + panel.w * 0.5, panel.y + panel.h * 0.5 + 5.0 * s);
        text.queue("Pick a sound above to add a track", x, y, 15.0 * s, Weight::Regular, DIM, Align::Center);
    }

    queue_chips(text, layout, frame);

    // Status line: a recent message, otherwise a pointer to the help.
    let (sx, sy) = layout.status_anchor;
    match &frame.status {
        Some((message, alpha)) => {
            let color = [BRIGHT[0], BRIGHT[1], BRIGHT[2], alpha.clamp(0.0, 1.0)];
            let message = fit(text, message, 13.0 * s, layout.status_width);
            text.queue(&message, sx, sy, 13.0 * s, Weight::Regular, color, Align::Center);
        }
        None if !frame.touch => {
            text.queue("Press F1 for help", sx, sy, 13.0 * s, Weight::Regular, FAINT, Align::Center);
        }
        None => {}
    }
}

fn queue_chips(text: &mut TextRenderer, layout: &Layout, frame: &FrameState) {
    let s = layout.scale;
    if let Some((x, y1, y2)) = layout.row_titles {
        text.queue("Patterns", x, y1, 12.0 * s, Weight::Regular, FAINT, Align::Left);
        text.queue("Sounds", x, y2, 12.0 * s, Weight::Regular, FAINT, Align::Left);
    }
    let size = if layout.compact { 12.5 } else { 13.5 } * s;
    for slot in layout.chips() {
        let r = slot.rect;
        let (cx, baseline) = (r.x + r.w * 0.5, r.y + r.h * 0.5 + size * 0.36);
        match slot.chip {
            Chip::Pattern(p) => {
                let color = if p == frame.selected_pattern { BRIGHT } else { DIM };
                text.queue(&(p + 1).to_string(), cx, baseline, size, Weight::Bold, color, Align::Center);
            }
            Chip::AddPattern => {
                text.queue("+", cx, baseline, size * 1.2, Weight::Bold, DIM, Align::Center);
            }
            Chip::Sharing => {
                let label = if frame.shared_tracks { "Shared tracks" } else { "Own tracks" };
                text.queue(label, cx, baseline, size, Weight::Regular, DIM, Align::Center);
            }
            Chip::Sound(n) => {
                if let Some((label, color)) = frame.sounds.get(n) {
                    // Colored chips have a dot on the left; move the text over.
                    let x = if color.is_some() { cx + 5.0 * s } else { cx };
                    text.queue(label, x, baseline, size, Weight::Regular, BRIGHT, Align::Center);
                }
            }
        }
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

/// The note shown once per version on Android, in Refraktal's own words.
const NOTICE: [&str; 8] = [
    "Refraktal is free software, made by an independent developer and",
    "published without Google's developer verification.",
    "Google now wants every Android developer to register their identity",
    "before apps can be installed the usual way. This began in four",
    "countries in September 2026; Google plans to go worldwide in 2027.",
    "I disagree: what runs on your own phone should be up to you.",
    "If an update is ever blocked, install it with adb, or allow apps from",
    "unverified developers in Developer options. keepandroidopen.org",
];

fn queue_notice(text: &mut TextRenderer, layout: &Layout, frame: &FrameState) {
    let s = layout.scale;
    let panel = layout.help;
    let left = panel.x + if layout.compact { 28.0 } else { 44.0 } * s;
    let (title_y, first_row) = if layout.compact { (40.0, 72.0) } else { (58.0, 108.0) };
    text.queue("A word about Android", left, panel.y + title_y * s, 24.0 * s, Weight::Bold, BRIGHT, Align::Left);
    for (i, line) in NOTICE.iter().enumerate() {
        let y = panel.y + first_row * s + i as f32 * layout.help_row;
        text.queue(line, left, y, 15.0 * s, Weight::Regular, DIM, Align::Left);
    }
    let footer = if frame.touch { "Tap anywhere to continue" } else { "Click anywhere to continue" };
    let footer_y = panel.y + panel.h - if layout.compact { 18.0 } else { 30.0 } * s;
    text.queue(footer, left, footer_y, 13.0 * s, Weight::Bold, BRIGHT, Align::Left);
}

/// Shorten `message` with an ellipsis until it fits in `width` pixels.
fn fit(text: &TextRenderer, message: &str, size: f32, width: f32) -> String {
    if text.measure(message, size, Weight::Regular) <= width {
        return message.to_owned();
    }
    let mut chars: Vec<char> = message.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let short: String = chars.iter().collect::<String>() + "…";
        if text.measure(&short, size, Weight::Regular) <= width {
            return short;
        }
    }
    String::new()
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
