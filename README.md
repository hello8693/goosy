# GoosyRenderer
Goosy renders an LRC or TTML lyric track into a scrolling lyric video. Landscape output supports an optional cover-art background, while portrait output keeps the full width for lyrics. AMLL-style translations render below the main line at `max(0.5em, 10px)` with a 1.5em translation line height. The active line is spring-animated, scaled from its left edge, and progressively filled from left to right. Output defaults to 1920×1080 at 30 fps and includes the source audio.

## Requirements

- Rust toolchain
- `ffmpeg` and `ffprobe` on `PATH` (the renderer streams RGBA frames to the FFmpeg CLI and uses FFprobe for audio duration)
- macOS system font `PingFang SC` for the default Chinese-capable text layout

## Usage

```sh
goosy render song.mp3 lyrics.lrc -o lyrics.mp4
```

Render video without an audio stream:

```sh
goosy render song.mp3 lyrics.lrc -o lyrics-only.mp4 --no-audio
```

FLAC audio is supported through the same FFmpeg path as MP3 and other FFmpeg-readable audio formats:

```sh
goosy render song.flac lyrics.ttml -o lyrics.mp4
```

Use a cover image as the animated background:

```sh
goosy render song.flac lyrics.ttml -o lyrics.mp4 --background cover.png
```

The repository includes `assets/zoo.flac` and `assets/zoo.ttml` for an end-to-end real-song check.

## Parameters

| Option | Default | Description |
| --- | --- | --- |
| `song` | required | Source audio file; still required with `--no-audio` for a stable command shape |
| `lyrics` | required | LRC or TTML/XML lyric file; auto-detected from TTML root `<tt>` (or `.ttml` extension) |
| `-o, --output` | `out.mp4` | Output MP4 path |
| `--width` | `1920` | Video width in pixels |
| `--height` | `1080` | Video height in pixels |
| `--fps` | `30` | Video frame rate |
| `--format` | `auto` | Force `auto`, `lrc`, or `ttml` when the input extension/content is ambiguous |
| `--background` | none | Optional cover image path; the image is cover-cropped, blurred, tinted, and masked for lyric readability |
| `--no-audio` | off | Omit the source audio stream |

Standard LRC timestamps (`[mm:ss]` and `[mm:ss.xx]`), repeated timestamps, metadata tags, and enhanced word timestamps (`<mm:ss.xx>word</mm:ss.xx>`) are accepted. TTML `<p>` lines with `begin`/`end`, timed `<span>` words, inline `ttm:role="x-translation"` translations, and AMLL sidecar translations keyed by `itunes:key` are accepted. Namespaced `xml:begin`/`xml:end` attributes are supported. LRC empty lyric lines remain in the timing/layout model but are not drawn; empty TTML paragraphs are discarded.

## Layout and background modules

`FrameGeometry` defines the frame, reserved cover viewport, and lyric viewport. `Renderer` composes a GPU-first `SurfaceRenderer`, `BackgroundRenderer`, and `LyricsRenderer`; background effects implement `BackgroundLayer` and receive the frame geometry plus presentation time, so animated gradients, cover art, blur, and overlays can be added without coupling them to lyric layout. Metal is selected by default on macOS, with raster fallback if device/context creation fails.

The repository does not force a machine-specific compiler path. If a local toolchain needs an override, set `CC`, `CXX`, `AR`, or the Cargo target linker environment variable in the developer shell or CI configuration.
## Motion and frame rate

The scroll uses a deliberately soft position spring (`stiffness=40`, `damping=10`) and is sampled once per output frame. At 10 fps, any animation is necessarily represented by 100 ms steps; use the default 30 fps or 60 fps when smooth scrolling matters. Lowering the frame rate does not create intermediate frames.

## Roadmap

- T3: Unicode word/grapheme segmentation, emphasized-word spring motion, and glow for word-level karaoke highlighting.
- T4: AMLL dynamic cover-art backgrounds, blur, animated overlays, translation, transliteration, harmony, and background lines.
- T5: Metal GPU rendering is enabled; remaining work is GPU readback optimization, parallel frame rendering, 4K/60fps support, and optional FFmpeg library integration.
