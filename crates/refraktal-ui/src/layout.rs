// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen layout and hit testing. All values are in physical pixels.

/// Number of steps shown in the sequencer.
pub const STEPS: usize = 16;
/// Most tracks the sequencer can show.
pub const MAX_TRACKS: usize = 8;
/// Most patterns the pattern row can show.
pub const MAX_PATTERNS: usize = 16;
/// Most sounds the sound row can show.
pub const MAX_SOUNDS: usize = 6;
/// Pattern chips, "+", the sharing switch and the sound chips.
pub const MAX_CHIPS: usize = MAX_PATTERNS + 2 + MAX_SOUNDS;

/// An axis-aligned rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    #[must_use]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    #[must_use]
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    None,
    Play,
    TempoDown,
    TempoUp,
    /// The "?" button that opens the help.
    Help,
    /// The export button in the transport pill.
    Export,
    /// A chip in the pattern or sound row.
    Chip(Chip),
    /// The colored marker at the start of a track row.
    Track(usize),
    Step { track: usize, step: usize },
}

/// The chips above the sequencer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// Select a pattern (right click or long press deletes it).
    Pattern(usize),
    /// Add a pattern.
    AddPattern,
    /// Switch between shared and per-pattern tracks.
    Sharing,
    /// Add a track with this sound, by index in the sound row.
    Sound(usize),
}

/// A chip and where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipSlot {
    pub chip: Chip,
    pub rect: Rect,
}

/// What the screen has to fit, besides its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Content {
    /// Tracks in the shown pattern; zero is allowed.
    pub tracks: usize,
    pub patterns: usize,
    pub sounds: usize,
}

/// Positions of every element on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    /// Tracks shown; may be zero (an empty pattern).
    pub track_count: usize,
    /// Glass pill holding the transport controls.
    pub transport: Rect,
    /// Glass panel holding the step grid.
    pub sequencer: Rect,
    /// Widest a status message may be.
    pub status_width: f32,
    /// Glass panel holding the pattern and sound rows.
    pub browser: Rect,
    /// Chips in the browser panel; the first `chip_count` are used.
    pub chips: [ChipSlot; MAX_CHIPS],
    pub chip_count: usize,
    /// Left edge of the row titles and the baselines of the two rows, if
    /// there is room for titles.
    pub row_titles: Option<(f32, f32, f32)>,
    pub panel_radius: f32,
    pub play_center: (f32, f32),
    pub play_radius: f32,
    /// Playhead dots: first x, last x, y, radius.
    pub dots: [f32; 4],
    /// Top-left corner of the step grid.
    pub grid_origin: (f32, f32),
    pub cell: f32,
    pub gap: f32,
    pub beat_gap: f32,
    pub row_gap: f32,
    /// Horizontal center of the track color markers.
    pub label_x: f32,
    /// Left edge of the track names.
    pub name_x: f32,
    /// Right edge and baseline of the tempo readout in the transport pill.
    pub bpm_anchor: (f32, f32),
    /// Center x and baseline of the status line under the sequencer.
    pub status_anchor: (f32, f32),
    /// Help overlay panel.
    pub help: Rect,
    /// Tempo buttons: minus center x, plus center x, center y, radius.
    pub tempo_buttons: [f32; 4],
    /// Help button: center x, center y, radius.
    pub help_button: (f32, f32, f32),
    /// Export button: center x, center y, radius.
    pub export_button: (f32, f32, f32),
    /// Vertical distance between help rows.
    pub help_row: f32,
    /// Small screens (phones in landscape) get tighter spacing.
    pub compact: bool,
}

impl Layout {
    /// Lay out the interface for a surface of `width` × `height` pixels.
    #[must_use]
    pub fn compute(width: f32, height: f32, scale: f32, content: Content) -> Self {
        let track_count = content.tracks.min(MAX_TRACKS);
        // An empty pattern keeps the height of one row for its hint.
        let rows = track_count.max(1);
        let s = scale.max(0.5);
        // Phones in landscape have little height; tighten everything.
        let compact = height / s < 560.0;
        let k = |normal: f32, small: f32| if compact { small * s } else { normal * s };
        let margin = k(32.0, 10.0);
        let pad = k(28.0, 16.0);
        // Room for the color marker and the track name.
        let label_w = 96.0 * s;
        let gap = k(8.0, 6.0);
        let beat_gap = k(22.0, 14.0);
        let row_gap = k(12.0, 6.0);

        // Browser panel: two rows of chips.
        let chip_h = k(30.0, 24.0);
        let chip_gap = k(6.0, 5.0);
        let browser_pad = k(12.0, 6.0);
        let browser_h = 2.0 * browser_pad + 2.0 * chip_h + chip_gap;

        // Panel width = padding + labels + 16 cells + gaps between them.
        let fixed = 2.0 * pad + label_w + 12.0 * gap + 3.0 * beat_gap;
        let available = (width - 2.0 * margin).min(1180.0 * s);
        let cell_by_width = (available - fixed) / STEPS as f32;

        let pill_h = k(64.0, 48.0);
        let stack_gap = k(24.0, 8.0);
        let fixed_h = 2.0 * margin + pill_h + browser_h + 2.0 * stack_gap + 2.0 * pad + (rows - 1) as f32 * row_gap;
        let cell_by_height = (height - fixed_h) / rows as f32;
        let cell = cell_by_width.min(cell_by_height).clamp(14.0 * s, 60.0 * s);

        let grid_w = STEPS as f32 * cell + 12.0 * gap + 3.0 * beat_gap;
        let panel_w = 2.0 * pad + label_w + grid_w;
        let panel_h = 2.0 * pad + rows as f32 * cell + (rows - 1) as f32 * row_gap;

        // Chip sizes and the width the rows need.
        let pattern_w = k(34.0, 28.0);
        let sound_w = k(72.0, 62.0);
        let sharing_w = k(128.0, 104.0);
        let title_w = 84.0 * s;
        let patterns = content.patterns.clamp(1, MAX_PATTERNS);
        let sounds = content.sounds.min(MAX_SOUNDS);
        let pattern_chips = patterns + usize::from(patterns < MAX_PATTERNS);
        let row1_w = pattern_chips as f32 * (pattern_w + chip_gap) + chip_gap * 2.0 + sharing_w;
        let row2_w = sounds as f32 * (sound_w + chip_gap);
        let chips_w = row1_w.max(row2_w) + 2.0 * browser_pad;
        // Titles ("Patterns", "Sounds") only when there is room for them.
        let titles = !compact && chips_w + title_w <= available;
        let needed_w = chips_w + if titles { title_w } else { 0.0 };
        let browser_w = panel_w.max(needed_w).min(width - 2.0 * margin);

        let pill_w = (panel_w * 0.66).clamp(540.0 * s, 720.0 * s).min(panel_w.max(browser_w));
        let total_h = pill_h + stack_gap + browser_h + stack_gap + panel_h;
        let top = ((height - total_h) * 0.42).max(margin);
        let cx = width * 0.5;

        let transport = Rect { x: cx - pill_w * 0.5, y: top, w: pill_w, h: pill_h };
        let browser = Rect { x: cx - browser_w * 0.5, y: top + pill_h + stack_gap, w: browser_w, h: browser_h };
        let sequencer = Rect {
            x: cx - panel_w * 0.5,
            y: browser.y + browser_h + stack_gap,
            w: panel_w,
            h: panel_h,
        };

        // Chips: patterns, "+", then the sharing switch at the right end;
        // sounds on the second row.
        let empty = ChipSlot { chip: Chip::AddPattern, rect: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 } };
        let mut chips = [empty; MAX_CHIPS];
        let mut chip_count = 0;
        let mut push = |chip: Chip, rect: Rect| {
            chips[chip_count] = ChipSlot { chip, rect };
            chip_count += 1;
        };
        let left = browser.x + browser_pad + if titles { title_w } else { 0.0 };
        let row1_y = browser.y + browser_pad;
        let row2_y = row1_y + chip_h + chip_gap;
        for i in 0..pattern_chips {
            let chip = if i < patterns { Chip::Pattern(i) } else { Chip::AddPattern };
            push(chip, Rect { x: left + i as f32 * (pattern_w + chip_gap), y: row1_y, w: pattern_w, h: chip_h });
        }
        let sharing_x = browser.x + browser.w - browser_pad - sharing_w;
        push(Chip::Sharing, Rect { x: sharing_x, y: row1_y, w: sharing_w, h: chip_h });
        for i in 0..sounds {
            push(Chip::Sound(i), Rect { x: left + i as f32 * (sound_w + chip_gap), y: row2_y, w: sound_w, h: chip_h });
        }
        let title_size = chip_h * 0.5 + 4.5 * s;
        let row_titles = titles.then_some((browser.x + browser_pad + 4.0 * s, row1_y + title_size, row2_y + title_size));

        let play_radius = k(22.0, 19.0);
        let play_center = (transport.x + pill_h * 0.5, transport.y + pill_h * 0.5);
        let right = transport.x + pill_w;
        let dots = [
            play_center.0 + play_radius + 24.0 * s,
            right - 258.0 * s,
            play_center.1,
            3.0 * s,
        ];

        let grid_origin = (sequencer.x + pad + label_w, sequencer.y + pad);
        let label_x = sequencer.x + pad + 8.0 * s;

        Self {
            scale: s,
            track_count,
            transport,
            sequencer,
            browser,
            chips,
            chip_count,
            row_titles,
            panel_radius: 26.0 * s,
            play_center,
            play_radius,
            dots,
            grid_origin,
            cell,
            gap,
            beat_gap,
            row_gap,
            label_x,
            name_x: label_x + 16.0 * s,
            bpm_anchor: (right - 136.0 * s, transport.y + pill_h * 0.5 + 7.0 * s),
            // Phones have no room below the grid (and system bars may cover
            // it), so messages replace the playhead dots in the pill instead.
            status_anchor: if compact {
                ((dots[0] + dots[1]) * 0.5, transport.y + pill_h * 0.5 + 4.5 * s)
            } else {
                (cx, sequencer.y + panel_h + k(36.0, 20.0))
            },
            status_width: if compact { dots[1] - dots[0] + 24.0 * s } else { sequencer.w },
            help: help_rect(width, height, s, compact),
            tempo_buttons: [right - 226.0 * s, right - 114.0 * s, play_center.1, 13.0 * s],
            help_button: (right - 32.0 * s, play_center.1, 15.0 * s),
            export_button: (right - 72.0 * s, play_center.1, 15.0 * s),
            help_row: k(30.0, 22.0),
            compact,
        }
    }

    /// The chips in use.
    #[must_use]
    pub fn chips(&self) -> &[ChipSlot] {
        &self.chips[..self.chip_count]
    }

    /// Baseline for text vertically centered on a track row.
    #[must_use]
    pub fn row_baseline(&self, track: usize) -> f32 {
        self.marker_center(track).1 + 4.5 * self.scale
    }

    /// Rectangle of one step cell.
    #[must_use]
    pub fn cell_rect(&self, track: usize, step: usize) -> Rect {
        let beat_stride = 4.0 * self.cell + 3.0 * self.gap + self.beat_gap;
        let x = self.grid_origin.0
            + (step / 4) as f32 * beat_stride
            + (step % 4) as f32 * (self.cell + self.gap);
        let y = self.grid_origin.1 + track as f32 * (self.cell + self.row_gap);
        Rect { x, y, w: self.cell, h: self.cell }
    }

    /// Center of a track's color marker.
    #[must_use]
    pub fn marker_center(&self, track: usize) -> (f32, f32) {
        let row = self.cell_rect(track, 0);
        (self.label_x, row.y + row.h * 0.5)
    }

    /// Find the control under the point `(x, y)`.
    #[must_use]
    pub fn hit_test(&self, x: f32, y: f32) -> Hit {
        let (pcx, pcy) = self.play_center;
        if (x - pcx).hypot(y - pcy) <= self.play_radius {
            return Hit::Play;
        }
        // Small buttons get a slightly larger touch area than they look.
        let slack = 6.0 * self.scale;
        let [minus_x, plus_x, ty, tr] = self.tempo_buttons;
        if (x - minus_x).hypot(y - ty) <= tr + slack {
            return Hit::TempoDown;
        }
        if (x - plus_x).hypot(y - ty) <= tr + slack {
            return Hit::TempoUp;
        }
        let (hx, hy, hr) = self.help_button;
        if (x - hx).hypot(y - hy) <= hr + slack {
            return Hit::Help;
        }
        let (ex, ey, er) = self.export_button;
        if (x - ex).hypot(y - ey) <= er + slack {
            return Hit::Export;
        }
        if self.browser.contains(x, y) {
            // Chips are short; let the touch area reach into the gaps around them.
            let reach = 3.0 * self.scale;
            for slot in self.chips() {
                let r = slot.rect;
                if x >= r.x - reach && x <= r.x + r.w + reach && y >= r.y - reach && y <= r.y + r.h + reach {
                    return Hit::Chip(slot.chip);
                }
            }
            return Hit::None;
        }
        if self.sequencer.contains(x, y) {
            for track in 0..self.track_count {
                let (mx, my) = self.marker_center(track);
                if (x - mx).hypot(y - my) <= 14.0 * self.scale {
                    return Hit::Track(track);
                }
            }
            for track in 0..self.track_count {
                for step in 0..STEPS {
                    if self.cell_rect(track, step).contains(x, y) {
                        return Hit::Step { track, step };
                    }
                }
            }
        }
        Hit::None
    }
}

/// Rows in the keyboard help overlay; the panel is sized to fit them.
pub(crate) const HELP_ROWS: usize = 14;
/// Rows in the touch help, which small (phone) screens show.
pub(crate) const TOUCH_HELP_ROWS: usize = 10;

fn help_rect(width: f32, height: f32, s: f32, compact: bool) -> Rect {
    let w = (680.0 * s).min(width - 48.0 * s);
    let h = if compact {
        64.0 * s + TOUCH_HELP_ROWS as f32 * 22.0 * s + 40.0 * s
    } else {
        96.0 * s + HELP_ROWS as f32 * 30.0 * s + 56.0 * s
    };
    Rect { x: (width - w) * 0.5, y: ((height - h) * 0.5).max(8.0 * s), w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(tracks: usize, patterns: usize) -> Content {
        Content { tracks, patterns, sounds: MAX_SOUNDS }
    }

    fn layouts() -> Vec<Layout> {
        let mut all = Vec::new();
        for tracks in 0..=MAX_TRACKS {
            for patterns in [1, 5, MAX_PATTERNS] {
                let c = content(tracks, patterns);
                all.push(Layout::compute(1280.0, 800.0, 1.0, c));
                all.push(Layout::compute(2560.0, 1600.0, 2.0, c));
                all.push(Layout::compute(960.0, 600.0, 1.0, c));
                all.push(Layout::compute(2400.0, 1080.0, 2.75, c));
            }
        }
        all
    }

    #[test]
    fn every_cell_center_hits_its_own_step() {
        for layout in layouts() {
            for track in 0..layout.track_count {
                for step in 0..STEPS {
                    let (x, y) = layout.cell_rect(track, step).center();
                    assert_eq!(layout.hit_test(x, y), Hit::Step { track, step });
                }
            }
        }
    }

    #[test]
    fn buttons_and_empty_space() {
        for layout in layouts() {
            let (x, y) = layout.play_center;
            assert_eq!(layout.hit_test(x, y), Hit::Play);
            let [minus_x, plus_x, ty, _] = layout.tempo_buttons;
            assert_eq!(layout.hit_test(minus_x, ty), Hit::TempoDown);
            assert_eq!(layout.hit_test(plus_x, ty), Hit::TempoUp);
            let (hx, hy, _) = layout.help_button;
            assert_eq!(layout.hit_test(hx, hy), Hit::Help);
            let (ex, ey, _) = layout.export_button;
            assert_eq!(layout.hit_test(ex, ey), Hit::Export);
            assert_eq!(layout.hit_test(1.0, 1.0), Hit::None);
            for track in 0..layout.track_count {
                let (x, y) = layout.marker_center(track);
                assert_eq!(layout.hit_test(x, y), Hit::Track(track));
            }
        }
    }

    #[test]
    fn an_empty_pattern_has_no_cells_to_hit() {
        let layout = Layout::compute(1280.0, 800.0, 1.0, content(0, 1));
        let (x, y) = layout.cell_rect(0, 3).center();
        assert_eq!(layout.hit_test(x, y), Hit::None);
    }

    #[test]
    fn every_chip_hits_itself_and_fits_its_panel() {
        for layout in layouts() {
            let chips = layout.chips();
            assert_eq!(chips.iter().filter(|c| matches!(c.chip, Chip::Sound(_))).count(), MAX_SOUNDS);
            assert!(chips.iter().any(|c| c.chip == Chip::Sharing));
            for slot in chips {
                let (x, y) = slot.rect.center();
                assert_eq!(layout.hit_test(x, y), Hit::Chip(slot.chip), "{slot:?}");
                let r = slot.rect;
                assert!(layout.browser.contains(r.x, r.y) && layout.browser.contains(r.x + r.w, r.y + r.h), "{slot:?}");
            }
            // Chips never overlap.
            for (i, a) in chips.iter().enumerate() {
                for b in &chips[i + 1..] {
                    let (ra, rb) = (a.rect, b.rect);
                    let apart = ra.x + ra.w <= rb.x || rb.x + rb.w <= ra.x || ra.y + ra.h <= rb.y || rb.y + rb.h <= ra.y;
                    assert!(apart, "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_add_pattern_chip_disappears_when_full() {
        let full = Layout::compute(1280.0, 800.0, 1.0, content(3, MAX_PATTERNS));
        assert!(full.chips().iter().all(|c| c.chip != Chip::AddPattern));
        let room = Layout::compute(1280.0, 800.0, 1.0, content(3, 2));
        assert!(room.chips().iter().any(|c| c.chip == Chip::AddPattern));
    }

    #[test]
    fn panels_are_stacked_without_overlap() {
        for layout in layouts() {
            assert!(layout.transport.y + layout.transport.h <= layout.browser.y);
            assert!(layout.browser.y + layout.browser.h <= layout.sequencer.y);
        }
    }

    #[test]
    fn grid_fits_inside_the_panel() {
        for layout in layouts() {
            if layout.track_count == 0 {
                continue;
            }
            let first = layout.cell_rect(0, 0);
            let last = layout.cell_rect(layout.track_count - 1, STEPS - 1);
            let panel = layout.sequencer;
            assert!(panel.contains(first.x, first.y));
            assert!(panel.contains(last.x + last.w, last.y + last.h));
        }
    }

    #[test]
    fn eight_tracks_fit_a_phone_in_landscape() {
        // A typical phone: 2400 × 1080 pixels at scale 2.75.
        let layout = Layout::compute(2400.0, 1080.0, 2.75, content(MAX_TRACKS, MAX_PATTERNS));
        assert!(layout.compact);
        assert!(layout.sequencer.y + layout.sequencer.h <= 1080.0);
        assert!(layout.browser.x >= 0.0 && layout.browser.x + layout.browser.w <= 2400.0);
        assert!(layout.help.y + layout.help.h <= 1080.0);
        assert!(layout.cell >= 20.0 * 2.75, "cells too small to tap: {}", layout.cell);
    }

    #[test]
    fn phone_messages_stay_on_screen() {
        let layout = Layout::compute(2400.0, 1080.0, 2.75, content(MAX_TRACKS, MAX_PATTERNS));
        let (x, y) = layout.status_anchor;
        assert!(layout.transport.contains(x, y), "the message must sit in the transport pill");
        assert!(layout.status_width > 100.0 * 2.75, "room for a short message");
    }

    #[test]
    fn eight_tracks_fit_the_smallest_window() {
        let layout = Layout::compute(960.0, 600.0, 1.0, content(MAX_TRACKS, MAX_PATTERNS));
        for panel in [layout.sequencer, layout.browser, layout.transport] {
            assert!(panel.x >= 0.0 && panel.x + panel.w <= 960.0, "{panel:?}");
            assert!(panel.y + panel.h <= 600.0, "{panel:?}");
        }
        assert!(layout.help.y + layout.help.h <= 600.0);
    }
}
