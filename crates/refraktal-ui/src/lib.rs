// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU-rendered glass interface for Refraktal.
//!
//! Rendering happens in three stages:
//!
//! 1. the animated neon backdrop is drawn into an HDR texture;
//! 2. that texture is blurred with a dual Kawase filter;
//! 3. a composite pass draws the glass panels, which refract and frost the
//!    backdrop behind them, and the controls on top.
//!
//! This crate knows nothing about audio. It receives plain data through
//! [`FrameState`] and reports what the pointer hit through [`Layout::hit_test`].

mod layout;
mod renderer;

pub use layout::{Hit, Layout, MAX_TRACKS, Rect, STEPS};
pub use renderer::{FrameState, Renderer};
