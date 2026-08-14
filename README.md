# GoosyRenderer

基于 Rust 和 Skia 的歌词视频渲染工具。

## 模块

- `src/main.rs`：入口
- `src/lib.rs`：核心 API
- `src/renderer.rs`：画面合成
- `src/lyrics_renderer.rs`：歌词排版与绘制
- `src/background.rs`、`src/cover_renderer.rs`：背景与封面
- `src/lrc.rs`、`src/ttml.rs`、`src/yrc.rs`：歌词解析
- `src/video.rs`、`src/pdf_renderer.rs`：视频与 PDF 输出
- `node/`、`python/`、`cpp/`、`include/`：绑定库
