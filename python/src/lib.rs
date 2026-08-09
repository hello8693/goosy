use goosy::{LyricFormat, RenderOptions};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::path::PathBuf;

fn error<E: std::fmt::Display>(value: E) -> PyErr {
    PyRuntimeError::new_err(value.to_string())
}

fn format_from_string(format: &str) -> PyResult<LyricFormat> {
    match format.to_ascii_lowercase().as_str() {
        "auto" => Ok(LyricFormat::Auto),
        "lrc" => Ok(LyricFormat::Lrc),
        "ttml" | "xml" => Ok(LyricFormat::Ttml),
        other => Err(PyValueError::new_err(format!(
            "unsupported lyric format: {other}"
        ))),
    }
}

#[pyfunction]
#[pyo3(signature = (song, output, lyrics=None, width=1920, height=1080, fps=30, background=None, cover=None, title=None, no_embedded_cover=false, no_audio=false, format="auto"))]
fn render(
    song: String,
    output: String,
    lyrics: Option<String>,
    width: u32,
    height: u32,
    fps: u32,
    background: Option<String>,
    cover: Option<String>,
    title: Option<String>,
    no_embedded_cover: bool,
    no_audio: bool,
    format: &str,
) -> PyResult<()> {
    let mut options = RenderOptions::new(song, output);
    options.lyrics = lyrics.map(PathBuf::from);
    options.width = width;
    options.height = height;
    options.fps = fps;
    options.background = background.map(PathBuf::from);
    options.cover = cover.map(PathBuf::from);
    options.title = title;
    options.no_embedded_cover = no_embedded_cover;
    options.no_audio = no_audio;
    options.format = format_from_string(format)?;
    goosy::render(&options).map_err(error)
}

fn words<'py>(
    py: Python<'py>,
    output: &Bound<'py, PyDict>,
    key: &str,
    source: &[goosy::LyricWord],
) -> PyResult<()> {
    let list = PyList::empty(py);
    for word in source {
        let item = PyDict::new(py);
        item.set_item("start_ms", word.start_ms)?;
        item.set_item("end_ms", word.end_ms)?;
        item.set_item("text", &word.text)?;
        list.append(item)?;
    }
    output.set_item(key, list)
}

fn line_to_py<'py>(py: Python<'py>, line: &goosy::LyricLine) -> PyResult<Py<PyAny>> {
    let output = PyDict::new(py);
    output.set_item("start_ms", line.start_ms)?;
    output.set_item("end_ms", line.end_ms)?;
    output.set_item("text", &line.text)?;
    output.set_item("translation", line.translation.as_deref())?;
    output.set_item("agent_id", line.agent_id.as_deref())?;
    output.set_item("is_duet", line.is_duet)?;
    output.set_item("is_background", line.is_background)?;
    words(py, &output, "words", &line.words)?;
    if let Some(background) = &line.background_vocal {
        let value = PyDict::new(py);
        value.set_item("start_ms", background.start_ms)?;
        value.set_item("end_ms", background.end_ms)?;
        value.set_item("text", &background.text)?;
        value.set_item("translation", background.translation.as_deref())?;
        words(py, &value, "words", &background.words)?;
        output.set_item("background_vocal", value)?;
    } else {
        output.set_item("background_vocal", py.None())?;
    }
    Ok(output.into_any().unbind())
}

#[pyfunction]
#[pyo3(signature = (input, format="auto"))]
fn parse_lyrics(py: Python<'_>, input: &str, format: &str) -> PyResult<Py<PyAny>> {
    let lines = goosy::parse_lyrics(input, format_from_string(format)?).map_err(error)?;
    let output = PyList::empty(py);
    for line in &lines {
        output.append(line_to_py(py, line)?)?;
    }
    Ok(output.into_any().unbind())
}

#[pymodule]
fn pygoosy(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(render, module)?)?;
    module.add_function(wrap_pyfunction!(parse_lyrics, module)?)?;
    Ok(())
}
