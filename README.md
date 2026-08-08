# GoosyRenderer

Goosy renders an LRC lyric track into a horizontal scrolling lyric video. The active line is centered, spring-animated, scaled up, and progressively filled from left to right. Output defaults to 1920×1080 at 30 fps and includes the source audio.

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

## Parameters

| Option | Default | Description |
| --- | --- | --- |
| `song` | required | Source audio file; still required with `--no-audio` for a stable command shape |
| `lyrics` | required | LRC lyric file |
| `-o, --output` | `out.mp4` | Output MP4 path |
| `--width` | `1920` | Video width in pixels |
| `--height` | `1080` | Video height in pixels |
| `--fps` | `30` | Video frame rate |
| `--no-audio` | off | Omit the source audio stream |

Standard LRC timestamps (`[mm:ss]` and `[mm:ss.xx]`), repeated timestamps, metadata tags, and enhanced word timestamps (`<mm:ss.xx>word</mm:ss.xx>`) are accepted. Empty lyric lines remain in the timing/layout model but are not drawn.
## Motion and frame rate

The scroll uses a deliberately soft position spring (`stiffness=40`, `damping=10`) and is sampled once per output frame. At 10 fps, any animation is necessarily represented by 100 ms steps; use the default 30 fps or 60 fps when smooth scrolling matters. Lowering the frame rate does not create intermediate frames.

## Roadmap

- T3: word-level karaoke highlighting using Unicode word/grapheme segmentation, emphasized-word spring motion, and glow.
- T4: blurred cover-art backgrounds plus translation, transliteration, harmony, and background lines.
- T5: Metal/GPU rendering, parallel frame rendering, 4K/60fps support, and optional FFmpeg library integration.
