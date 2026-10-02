// SPDX-License-Identifier: GPL-3.0-or-later
//! Android entry point. On other platforms this crate is empty.

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    if let Err(err) = refraktal::run_android(app) {
        eprintln!("Refraktal stopped: {err:#}");
    }
}
