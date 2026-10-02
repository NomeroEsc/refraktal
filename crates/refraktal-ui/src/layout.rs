// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen layout and hit testing. All values are in physical pixels.

/// Number of steps shown in the sequencer.
pub const STEPS: usize = 16;
/// Most tracks the sequencer can show.
pub const MAX_TRACKS: usize = 8;

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
    /// The "+" button under the last track.
    AddTrack,
    /// The colored marker at the start of a track row.
    Track(usize),
    Step { track: usize, step: usize },
}

/// Positions of every element on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub track_count: usize,
    /// Glass pill holding the transport controls.
    pub transport: Rect,
    /// Glass panel holding the step grid.
    pub sequencer: Rect,
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
    /// Center x, center y and radius of the "+" button, if there is room
    /// for another track.
    pub add_button: Option<(f32, f32, f32)>,
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
    /// Vertical distance between help rows.
    pub help_row: f32,
    /// Small screens (phones in landscape) get tighter spacing.
    pub compact: bool,
}

impl Layout {
    /// Lay out the interface for a surface of `width` × `height` pixels
    /// showing `track_count` tracks.
    #[must_use]
    pub fn compute(width: f32, height: f32, scale: f32, track_count: usize) -> Self {
        let rows = track_count.clamp(1, MAX_TRACKS);
        let s = scale.max(0.5);
        // Phones in landscape have little height; tighten everything.
        let compact = height / s < 560.0;
        let k = |normal: f32, small: f32| if compact { small * s } else { normal * s };
        let margin = k(32.0, 12.0);
        let pad = k(28.0, 16.0);
        // Room for the color marker and the track name.
        let label_w = 96.0 * s;
        let gap = k(8.0, 6.0);
        let beat_gap = k(22.0, 14.0);
        let row_gap = k(12.0, 6.0);

        // Panel width = padding + labels + 16 cells + gaps between them.
        let fixed = 2.0 * pad + label_w + 12.0 * gap + 3.0 * beat_gap;
        let available = (width - 2.0 * margin).min(1180.0 * s);
        let cell_by_width = (available - fixed) / STEPS as f32;

        let pill_h = k(64.0, 52.0);
        let stack_gap = k(24.0, 10.0);
        let add_h = if rows < MAX_TRACKS { row_gap + 28.0 * s } else { 0.0 };
        let fixed_h = 2.0 * margin + pill_h + stack_gap + 2.0 * pad + add_h + (rows - 1) as f32 * row_gap;
        let cell_by_height = (height - fixed_h) / rows as f32;
        let cell = cell_by_width.min(cell_by_height).clamp(14.0 * s, 60.0 * s);

        let grid_w = STEPS as f32 * cell + 12.0 * gap + 3.0 * beat_gap;
        let panel_w = 2.0 * pad + label_w + grid_w;
        let panel_h = 2.0 * pad + rows as f32 * cell + (rows - 1) as f32 * row_gap + add_h;

        let pill_w = (panel_w * 0.66).clamp(500.0 * s, 700.0 * s).min(panel_w);
        let total_h = pill_h + stack_gap + panel_h;
        let top = ((height - total_h) * 0.42).max(margin);
        let cx = width * 0.5;

        let transport = Rect { x: cx - pill_w * 0.5, y: top, w: pill_w, h: pill_h };
        let sequencer = Rect {
            x: cx - panel_w * 0.5,
            y: top + pill_h + stack_gap,
            w: panel_w,
            h: panel_h,
        };

        let play_radius = 22.0 * s;
        let play_center = (
            transport.x + (pill_h * 0.5 - play_radius) + play_radius,
            transport.y + pill_h * 0.5,
        );
        let right = transport.x + pill_w;
        let dots = [
            play_center.0 + play_radius + 24.0 * s,
            right - 214.0 * s,
            play_center.1,
            3.0 * s,
        ];

        let grid_origin = (sequencer.x + pad + label_w, sequencer.y + pad);
        let label_x = sequencer.x + pad + 8.0 * s;
        let add_button = (rows < MAX_TRACKS).then(|| {
            let last_bottom = grid_origin.1 + rows as f32 * (cell + row_gap) - row_gap;
            (label_x, last_bottom + row_gap + 14.0 * s, 11.0 * s)
        });

        Self {
            scale: s,
            track_count: rows,
            transport,
            sequencer,
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
            add_button,
            name_x: label_x + 16.0 * s,
            bpm_anchor: (right - 98.0 * s, transport.y + pill_h * 0.5 + 7.0 * s),
            status_anchor: (cx, sequencer.y + panel_h + k(36.0, 22.0)),
            help: help_rect(width, height, s, compact),
            tempo_buttons: [right - 186.0 * s, right - 76.0 * s, play_center.1, 13.0 * s],
            help_button: (right - 32.0 * s, play_center.1, 15.0 * s),
            help_row: k(30.0, 22.0),
            compact,
        }
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
        if let Some((ax, ay, r)) = self.add_button {
            if (x - ax).hypot(y - ay) <= r + 4.0 * self.scale {
                return Hit::AddTrack;
            }
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

/// Number of rows in the help overlay; the panel is sized to fit them.
pub(crate) const HELP_ROWS: usize = 12;

fn help_rect(width: f32, height: f32, s: f32, compact: bool) -> Rect {
    let w = (680.0 * s).min(width - 48.0 * s);
    let h = if compact {
        64.0 * s + HELP_ROWS as f32 * 22.0 * s + 40.0 * s
    } else {
        96.0 * s + HELP_ROWS as f32 * 30.0 * s + 56.0 * s
    };
    Rect { x: (width - w) * 0.5, y: ((height - h) * 0.5).max(8.0 * s), w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts() -> Vec<Layout> {
        let mut all = Vec::new();
        for tracks in 1..=MAX_TRACKS {
            all.push(Layout::compute(1280.0, 800.0, 1.0, tracks));
            all.push(Layout::compute(2560.0, 1600.0, 2.0, tracks));
            all.push(Layout::compute(960.0, 600.0, 1.0, tracks));
            all.push(Layout::compute(2400.0, 1080.0, 2.75, tracks));
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
            assert_eq!(layout.hit_test(1.0, 1.0), Hit::None);
            for track in 0..layout.track_count {
                let (x, y) = layout.marker_center(track);
                assert_eq!(layout.hit_test(x, y), Hit::Track(track));
            }
            match layout.add_button {
                Some((x, y, _)) => {
                    assert!(layout.track_count < MAX_TRACKS);
                    assert_eq!(layout.hit_test(x, y), Hit::AddTrack);
                }
                None => assert_eq!(layout.track_count, MAX_TRACKS),
            }
        }
    }

    #[test]
    fn grid_fits_inside_the_panel() {
        for layout in layouts() {
            let first = layout.cell_rect(0, 0);
            let last = layout.cell_rect(layout.track_count - 1, STEPS - 1);
            let panel = layout.sequencer;
            assert!(panel.contains(first.x, first.y));
            assert!(panel.contains(last.x + last.w, last.y + last.h));
            if let Some((x, y, r)) = layout.add_button {
                assert!(panel.contains(x, y + r));
            }
        }
    }

    #[test]
    fn eight_tracks_fit_a_phone_in_landscape() {
        // A typical phone: 2400 × 1080 pixels at scale 2.75.
        let layout = Layout::compute(2400.0, 1080.0, 2.75, MAX_TRACKS);
        assert!(layout.compact);
        assert!(layout.sequencer.y + layout.sequencer.h <= 1080.0);
        assert!(layout.help.y + layout.help.h <= 1080.0);
        assert!(layout.cell >= 20.0 * 2.75, "cells too small to tap: {}", layout.cell);
    }

    #[test]
    fn eight_tracks_fit_the_smallest_window() {
        let layout = Layout::compute(960.0, 600.0, 1.0, MAX_TRACKS);
        assert!(layout.sequencer.x >= 0.0);
        assert!(layout.sequencer.x + layout.sequencer.w <= 960.0);
        assert!(layout.sequencer.y + layout.sequencer.h <= 600.0);
    }
}
