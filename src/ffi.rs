use crate::{LyricFormat, RenderOptions, parse_lyrics, render};
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

static LAST_ERROR: LazyLock<Mutex<CString>> =
    LazyLock::new(|| Mutex::new(CString::new("unknown Goosy error").unwrap()));

fn last_error() -> &'static Mutex<CString> {
    &LAST_ERROR
}

fn set_error(message: impl Into<String>) {
    let message = message.into().replace('\0', "\\0");
    if let Ok(mut error) = last_error().lock() {
        *error =
            CString::new(message).unwrap_or_else(|_| CString::new("unknown Goosy error").unwrap());
    }
}

unsafe fn read_string(value: *const c_char, name: &str) -> Result<String, String> {
    if value.is_null() {
        return Err(format!("{name} is required"));
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| format!("{name} must be UTF-8"))
}

fn value_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("request.{key} must be a string"))
}

fn optional_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| format!("request.{key} must be a string or null")),
    }
}

fn request_options(request: &str) -> Result<RenderOptions, String> {
    let value: serde_json::Value =
        serde_json::from_str(request).map_err(|error| format!("invalid request JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "request must be a JSON object".to_owned())?;
    let mut options = RenderOptions::new(
        value_string(object, "song")?,
        value_string(object, "output")?,
    );
    options.lyrics = optional_string(object, "lyrics")?.map(PathBuf::from);
    options.background = optional_string(object, "background")?.map(PathBuf::from);
    options.cover = optional_string(object, "cover")?.map(PathBuf::from);
    options.title = optional_string(object, "title")?;
    if let Some(value) = object.get("width") {
        options.width = value
            .as_u64()
            .ok_or_else(|| "request.width must be a positive integer".to_owned())?
            as u32;
    }
    if let Some(value) = object.get("height") {
        options.height = value
            .as_u64()
            .ok_or_else(|| "request.height must be a positive integer".to_owned())?
            as u32;
    }
    if let Some(value) = object.get("fps") {
        options.fps = value
            .as_u64()
            .ok_or_else(|| "request.fps must be a positive integer".to_owned())?
            as u32;
    }
    if let Some(value) = object.get("no_embedded_cover") {
        options.no_embedded_cover = value
            .as_bool()
            .ok_or_else(|| "request.no_embedded_cover must be boolean".to_owned())?;
    }
    if let Some(value) = object.get("no_audio") {
        options.no_audio = value
            .as_bool()
            .ok_or_else(|| "request.no_audio must be boolean".to_owned())?;
    }
    options.format = match object
        .get("format")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
    {
        "auto" => LyricFormat::Auto,
        "lrc" => LyricFormat::Lrc,
        "ttml" | "xml" => LyricFormat::Ttml,
        other => return Err(format!("unsupported lyric format: {other}")),
    };
    Ok(options)
}

fn word_json(word: &crate::LyricWord) -> serde_json::Value {
    serde_json::json!({"start_ms": word.start_ms, "end_ms": word.end_ms, "text": word.text})
}

fn line_json(line: &crate::LyricLine) -> serde_json::Value {
    serde_json::json!({
        "start_ms": line.start_ms,
        "end_ms": line.end_ms,
        "text": line.text,
        "translation": line.translation,
        "agent_id": line.agent_id,
        "is_duet": line.is_duet,
        "is_background": line.is_background,
        "background_vocal": line.background_vocal.as_ref().map(|background| serde_json::json!({
            "start_ms": background.start_ms,
            "end_ms": background.end_ms,
            "text": background.text,
            "translation": background.translation,
            "words": background.words.iter().map(word_json).collect::<Vec<_>>(),
        })),
        "words": line.words.iter().map(word_json).collect::<Vec<_>>(),
    })
}

fn allocated_string(value: String) -> *mut c_char {
    CString::new(value)
        .unwrap_or_else(|_| CString::new("\\0").unwrap())
        .into_raw()
}
static VERSION: LazyLock<CString> = LazyLock::new(|| {
    CString::new(concat!(
        env!("CARGO_PKG_NAME"),
        " ",
        env!("CARGO_PKG_VERSION")
    ))
    .unwrap()
});

#[unsafe(no_mangle)]
pub extern "C" fn goosy_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn goosy_last_error() -> *const c_char {
    last_error()
        .lock()
        .map(|error| error.as_ptr())
        .unwrap_or(std::ptr::null())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn goosy_free_string(value: *mut c_char) {
    if !value.is_null() {
        unsafe {
            drop(CString::from_raw(value));
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn goosy_render_json(request: *const c_char) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let request = unsafe { read_string(request, "request") }?;
        let options = request_options(&request)?;
        render(&options).map_err(|error| error.to_string())
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            set_error(error);
            1
        }
        Err(_) => {
            set_error("Goosy panicked while rendering");
            2
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn goosy_parse_lyrics_json(
    input: *const c_char,
    format: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let input = unsafe { read_string(input, "input") }?;
        let format = unsafe { read_string(format, "format") }?;
        let format = match format.as_str() {
            "auto" => LyricFormat::Auto,
            "lrc" => LyricFormat::Lrc,
            "ttml" | "xml" => LyricFormat::Ttml,
            other => return Err(format!("unsupported lyric format: {other}")),
        };
        let lines = parse_lyrics(&input, format).map_err(|error| error.to_string())?;
        serde_json::to_string(&lines.iter().map(line_json).collect::<Vec<_>>())
            .map_err(|error| error.to_string())
    }));
    match result {
        Ok(Ok(value)) => allocated_string(value),
        Ok(Err(error)) => {
            set_error(error);
            std::ptr::null_mut()
        }
        Err(_) => {
            set_error("Goosy panicked while parsing lyrics");
            std::ptr::null_mut()
        }
    }
}
