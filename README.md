# GoosyRenderer
Goosy renders an LRC or TTML lyric track into a scrolling lyric video. Landscape output uses a golden-ratio composition: the left 38.2% is reserved for optional cover art and the right 61.8% is dedicated to lyrics. Portrait output keeps the full width for lyrics. AMLL-style translations render below the main line at `max(0.5em, 10px)` with a 1.5em translation line height. The active line is spring-animated, scaled from its left edge, and progressively filled from left to right. Output defaults to 1920×1080 at 30 fps and includes the source audio.

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

Launch the graphical workflow for selecting an audio file, automatically discovering sibling lyrics and cover art, editing the title, and starting the render:

```sh
goosy gui
```

FLAC audio is supported through the same FFmpeg path as MP3 and other FFmpeg-readable audio formats:

```sh
goosy render song.flac lyrics.ttml -o lyrics.mp4
```

By default, Goosy extracts embedded artwork from the audio file, renders it as the square rounded cover in the left golden-ratio column, and uses it for the highly blurred animated wallpaper:

```sh
goosy render assets/zoo.flac assets/zoo.ttml -o zoo.mp4 --title "Zoo"
```

Use `--cover cover.png` to override embedded artwork, `--background wallpaper.png` to choose a different wallpaper, or `--no-embedded-cover` to disable automatic extraction.

The repository includes `assets/zoo.flac`, `assets/zoo.ttml`, and `assets/zoo.jpeg` for an end-to-end real-song check.

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
| `--background` | embedded cover | Optional wallpaper override; the image is cover-cropped, highly blurred, layered, tinted, and masked for lyric readability |
| `--cover` | embedded audio artwork | Optional cover override; accepts any image format decodable by Skia and renders a square center crop with rounded corners and shadow |
| `--no-embedded-cover` | off | Disable automatic embedded artwork extraction when no explicit cover is supplied |
| `--title`, `--song-name` | none | Song name rendered below the cover, centered and truncated to two lines |
| `--no-audio` | off | Omit the source audio stream |

Standard LRC timestamps (`[mm:ss]` and `[mm:ss.xx]`), repeated timestamps, metadata tags, and enhanced word timestamps (`<mm:ss.xx>word</mm:ss.xx>`) are accepted. TTML `<p>` lines with `begin`/`end`, timed `<span>` words, inline `ttm:role="x-translation"` translations, and AMLL sidecar translations keyed by `itunes:key` are accepted. Namespaced `xml:begin`/`xml:end` attributes are supported. LRC empty lyric lines remain in the timing/layout model but are not drawn; empty TTML paragraphs are discarded.

## Layout and background modules

`FrameGeometry` defines the frame, a 38.2% cover viewport, and a 61.8% lyric viewport in landscape mode. Embedded audio artwork is the default cover and animated wallpaper source; an explicit `--background` overrides only the wallpaper. `CoverRenderer`, `BackgroundRenderer`, and `LyricsRenderer` remain independent layers composed by the GPU-first `Renderer`. The cover uses a square center crop, rounded corners, a soft shadow, a subtle border, and centered two-line title typography. Metal is selected by default on macOS, with raster fallback if device/context creation fails.
The Cargo configuration uses the system `/usr/bin/clang` linker for macOS targets to avoid broken third-party compiler wrappers.
## Motion and frame rate

The scroll uses a deliberately soft position spring (`stiffness=40`, `damping=10`) and is sampled once per output frame. At 10 fps, any animation is necessarily represented by 100 ms steps; use the default 30 fps or 60 fps when smooth scrolling matters. Lowering the frame rate does not create intermediate frames.

## Roadmap

- T3: Unicode word/grapheme segmentation, emphasized-word spring motion, and glow for word-level karaoke highlighting.
- T4: AMLL dynamic cover-art backgrounds, blur, animated overlays, translation, transliteration, harmony, and background lines.
- T5: Metal GPU rendering is enabled; remaining work is GPU readback optimization, parallel frame rendering, 4K/60fps support, and optional FFmpeg library integration.
