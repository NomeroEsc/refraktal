// SPDX-License-Identifier: GPL-3.0-or-later
//! Saving exported files into the shared Downloads folder on Android.
//!
//! Android 10 (API 29) and newer: the file goes through MediaStore into
//! `Download/Refraktal`; no permission is needed, and the system share
//! sheet opens with it. Android 8 and 9: MediaStore has no Downloads
//! collection yet, so the file is written to the same folder directly,
//! which needs `WRITE_EXTERNAL_STORAGE` (declared for API 28 and lower only).
//!
//! Everything here calls Java through JNI. The tests compile this module on
//! every platform so mistakes in types show up without a phone; the JNI
//! signatures themselves can only be checked on a device.

// Off Android only the tests compile this module, and they use little of it.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use anyhow::{Context, Result, bail};
use jni::JNIEnv;
use jni::objects::{JObject, JString, JValue};

/// Subfolder of Downloads that exports go into.
pub const FOLDER: &str = "Refraktal";
const WRITE_STORAGE: &str = "android.permission.WRITE_EXTERNAL_STORAGE";
/// `Intent.FLAG_GRANT_READ_URI_PERMISSION`.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 1;
/// `PackageManager.PERMISSION_GRANTED`.
const PERMISSION_GRANTED: i32 = 0;

/// What happened to an export.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Saved; the text says where, for the status line.
    Saved(String),
    /// Android 8 or 9 asked the user for storage access; try again after
    /// they allow it.
    NeedsPermission,
}

/// Save `bytes` as `name` in Downloads/Refraktal and offer to share it.
/// Call from any thread; it attaches to the Java VM itself.
#[cfg(target_os = "android")]
pub fn save_to_downloads(name: &str, mime: &str, bytes: &[u8]) -> Result<Outcome> {
    let context = ndk_context::android_context();
    // SAFETY: android-activity fills in ndk-context with the process's VM
    // and a global reference to the activity, both valid for the app's life.
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast()) }.context("no Java VM")?;
    let mut env = vm.attach_current_thread().context("could not attach to Java")?;
    // SAFETY: see above; the reference is global and never deleted here.
    let activity = unsafe { JObject::from_raw(context.context().cast()) };
    let result = save_with(&mut env, &activity, name, mime, bytes);
    if env.exception_check().unwrap_or(false) {
        // Leave no Java exception pending, or the next JNI call would abort.
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result
}

/// The part of [`save_to_downloads`] that works on a given JNI environment.
pub fn save_with(env: &mut JNIEnv, activity: &JObject, name: &str, mime: &str, bytes: &[u8]) -> Result<Outcome> {
    let sdk = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|v| v.i())
        .context("could not read the Android version")?;
    if sdk >= 29 {
        let uri = save_media_store(env, activity, name, mime, bytes)?;
        // Saving worked; a share sheet that fails to open is not an error.
        if let Err(err) = share(env, activity, &uri, mime) {
            eprintln!("Could not open the share sheet: {err:#}");
            let _ = env.exception_clear();
        }
        Ok(Outcome::Saved(format!("Saved to Downloads/{FOLDER}")))
    } else {
        save_legacy(env, activity, name, bytes)
    }
}

fn save_media_store<'a>(
    env: &mut JNIEnv<'a>,
    activity: &JObject,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<JObject<'a>> {
    let resolver = env
        .call_method(activity, "getContentResolver", "()Landroid/content/ContentResolver;", &[])?
        .l()?;
    let values = env.new_object("android/content/ContentValues", "()V", &[])?;
    put_string(env, &values, "_display_name", name)?;
    put_string(env, &values, "mime_type", mime)?;
    put_string(env, &values, "relative_path", &format!("Download/{FOLDER}"))?;
    // Hidden from other apps until it is completely written.
    put_int(env, &values, "is_pending", 1)?;

    let collection = env
        .get_static_field("android/provider/MediaStore$Downloads", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?
        .l()?;
    let uri = env
        .call_method(
            &resolver,
            "insert",
            "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
            &[JValue::from(&collection), JValue::from(&values)],
        )
        .context("Android refused to create the file")?
        .l()?;
    if uri.is_null() {
        bail!("Android refused to create the file");
    }

    let written = write_to_uri(env, &resolver, &uri, bytes);
    if written.is_err() {
        // Do not leave an empty or half-written file behind.
        let _ = env.exception_clear();
        let null = JObject::null();
        let _ = env.call_method(
            &resolver,
            "delete",
            "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
            &[JValue::from(&uri), JValue::from(&null), JValue::from(&null)],
        );
        return written.map(|()| uri);
    }

    let done = env.new_object("android/content/ContentValues", "()V", &[])?;
    put_int(env, &done, "is_pending", 0)?;
    let null = JObject::null();
    env.call_method(
        &resolver,
        "update",
        "(Landroid/net/Uri;Landroid/content/ContentValues;Ljava/lang/String;[Ljava/lang/String;)I",
        &[JValue::from(&uri), JValue::from(&done), JValue::from(&null), JValue::from(&null)],
    )
    .context("could not finish the file")?;
    Ok(uri)
}

fn write_to_uri(env: &mut JNIEnv, resolver: &JObject, uri: &JObject, bytes: &[u8]) -> Result<()> {
    let stream = env
        .call_method(resolver, "openOutputStream", "(Landroid/net/Uri;)Ljava/io/OutputStream;", &[JValue::from(uri)])
        .context("could not open the file for writing")?
        .l()?;
    if stream.is_null() {
        bail!("could not open the file for writing");
    }
    let array = env.byte_array_from_slice(bytes)?;
    let wrote = env.call_method(&stream, "write", "([B)V", &[JValue::from(&array)]);
    let closed = env.call_method(&stream, "close", "()V", &[]);
    wrote.context("could not write the file")?;
    closed.context("could not finish writing the file")?;
    Ok(())
}

/// Android 8 and 9: write into the public Downloads folder directly.
fn save_legacy(env: &mut JNIEnv, activity: &JObject, name: &str, bytes: &[u8]) -> Result<Outcome> {
    let permission = env.new_string(WRITE_STORAGE)?;
    let state = env
        .call_method(activity, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::from(&permission)])?
        .i()?;
    if state != PERMISSION_GRANTED {
        let list = env.new_object_array(1, "java/lang/String", &permission)?;
        env.call_method(activity, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::from(&list), JValue::Int(1)])?;
        return Ok(Outcome::NeedsPermission);
    }

    let kind = env
        .get_static_field("android/os/Environment", "DIRECTORY_DOWNLOADS", "Ljava/lang/String;")?
        .l()?;
    let dir = env
        .call_static_method(
            "android/os/Environment",
            "getExternalStoragePublicDirectory",
            "(Ljava/lang/String;)Ljava/io/File;",
            &[JValue::from(&kind)],
        )?
        .l()?;
    let path = JString::from(env.call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?.l()?);
    let downloads: String = env.get_string(&path)?.into();

    let folder = std::path::Path::new(&downloads).join(FOLDER);
    std::fs::create_dir_all(&folder).with_context(|| format!("could not create Downloads/{FOLDER}"))?;
    let file = free_name(&folder, name);
    std::fs::write(&file, bytes).context("could not write the file")?;
    let shown = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Outcome::Saved(format!("Saved to Downloads/{FOLDER}/{shown}")))
}

/// `name`, or `name (2)`, `name (3)`… if it is taken.
fn free_name(folder: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut candidate = folder.join(name);
    let mut n = 2;
    while candidate.exists() {
        candidate = folder.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    candidate
}

/// Open the system share sheet for a content URI.
fn share(env: &mut JNIEnv, activity: &JObject, uri: &JObject, mime: &str) -> Result<()> {
    let action = env.get_static_field("android/content/Intent", "ACTION_SEND", "Ljava/lang/String;")?.l()?;
    let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::from(&action)])?;
    let mime = env.new_string(mime)?;
    env.call_method(&intent, "setType", "(Ljava/lang/String;)Landroid/content/Intent;", &[JValue::from(&mime)])?;
    let extra = env.get_static_field("android/content/Intent", "EXTRA_STREAM", "Ljava/lang/String;")?.l()?;
    env.call_method(
        &intent,
        "putExtra",
        "(Ljava/lang/String;Landroid/os/Parcelable;)Landroid/content/Intent;",
        &[JValue::from(&extra), JValue::from(uri)],
    )?;
    env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(FLAG_GRANT_READ_URI_PERMISSION)])?;
    let title = env.new_string("Share the beat")?;
    let chooser = env
        .call_static_method(
            "android/content/Intent",
            "createChooser",
            "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
            &[JValue::from(&intent), JValue::from(&title)],
        )?
        .l()?;
    env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::from(&chooser)])?;
    Ok(())
}

fn put_string(env: &mut JNIEnv, values: &JObject, key: &str, value: &str) -> Result<()> {
    let key = env.new_string(key)?;
    let value = env.new_string(value)?;
    env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::from(&key), JValue::from(&value)],
    )?;
    Ok(())
}

fn put_int(env: &mut JNIEnv, values: &JObject, key: &str, value: i32) -> Result<()> {
    let key = env.new_string(key)?;
    let boxed = env.new_object("java/lang/Integer", "(I)V", &[JValue::Int(value)])?;
    env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/Integer;)V",
        &[JValue::from(&key), JValue::from(&boxed)],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_names_count_up() {
        let dir = std::env::temp_dir().join(format!("refraktal-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_name(&dir, "beat.wav"), dir.join("beat.wav"));
        std::fs::write(dir.join("beat.wav"), b"").unwrap();
        assert_eq!(free_name(&dir, "beat.wav"), dir.join("beat (2).wav"));
        std::fs::write(dir.join("beat (2).wav"), b"").unwrap();
        assert_eq!(free_name(&dir, "beat.wav"), dir.join("beat (3).wav"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
