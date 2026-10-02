// SPDX-License-Identifier: GPL-3.0-or-later
//! Native dialogs on desktop; quiet fallbacks where there are none (Android).

use std::path::PathBuf;

/// What to do with unsaved changes.
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

#[cfg(not(target_os = "android"))]
mod imp {
    use super::{Answer, PathBuf};
    use refraktal_io::PROJECT_EXTENSION;
    use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

    pub fn pick_project_to_open() -> Option<PathBuf> {
        FileDialog::new()
            .set_title("Open project")
            .add_filter("Refraktal project", &[PROJECT_EXTENSION])
            .pick_file()
    }

    pub fn pick_project_to_save() -> Option<PathBuf> {
        FileDialog::new()
            .set_title("Save project")
            .add_filter("Refraktal project", &[PROJECT_EXTENSION])
            .set_file_name(format!("beat.{PROJECT_EXTENSION}"))
            .save_file()
    }

    pub fn ask_about_unsaved_changes() -> Answer {
        let answer = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("Unsaved changes")
            .set_description("Save changes to this project first?")
            .set_buttons(MessageButtons::YesNoCancel)
            .show();
        match answer {
            MessageDialogResult::Yes => Answer::Save,
            MessageDialogResult::No => Answer::Discard,
            _ => Answer::Cancel,
        }
    }

    pub fn show_error(title: &str, err: &anyhow::Error) {
        MessageDialog::new()
            .set_level(MessageLevel::Error)
            .set_title(title)
            .set_description(format!("{err:#}"))
            .set_buttons(MessageButtons::Ok)
            .show();
    }
}

#[cfg(target_os = "android")]
mod imp {
    use super::{Answer, PathBuf};

    pub fn pick_project_to_open() -> Option<PathBuf> {
        None
    }

    pub fn pick_project_to_save() -> Option<PathBuf> {
        None
    }

    /// The Android build saves automatically, so nothing is ever lost.
    pub fn ask_about_unsaved_changes() -> Answer {
        Answer::Discard
    }

    pub fn show_error(title: &str, err: &anyhow::Error) {
        eprintln!("{title}: {err:#}");
    }
}

pub use imp::*;
