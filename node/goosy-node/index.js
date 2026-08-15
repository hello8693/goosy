const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

function binaryCandidates() {
  const configured = process.env.GOOSY_BIN;
  const root = path.resolve(__dirname, '..', '..');
  return [
    configured,
    path.join(root, 'target', 'release', process.platform === 'win32' ? 'goosy.exe' : 'goosy'),
    path.join(root, 'target', 'debug', process.platform === 'win32' ? 'goosy.exe' : 'goosy'),
    'goosy',
  ].filter(Boolean);
}

function binary() {
  const value = binaryCandidates().find((candidate) => candidate === 'goosy' || fs.existsSync(candidate));
  if (!value) throw new Error('Goosy binary not found; build it with cargo build --release');
  return value;
}

function nativeCandidates() {
  return [
    process.env.GOOSY_NODE_NATIVE,
    path.join(__dirname, 'goosy_node.node'),
    path.join(__dirname, `goosy_node.${process.platform}-${process.arch}.node`),
  ].filter(Boolean);
}

function nativeBinding() {
  const candidate = nativeCandidates().find((value) => fs.existsSync(value));
  return candidate ? require(candidate) : null;
}

function renderCli(options) {
  if (!options || !options.song || !options.output) {
    throw new TypeError('render requires { song, output }');
  }
  const args = ['render', options.song];
  if (options.lyrics) args.push(options.lyrics);
  args.push('--output', options.output);
  for (const [key, defaultValue] of [['width', 1920], ['height', 1080], ['fps', 30]]) {
    args.push(`--${key}`, String(options[key] === undefined ? defaultValue : options[key]));
  }
  args.push('--font-scale', String(options.font_scale === undefined ? 1 : options.font_scale));
  for (const key of [
    'line_height_scale',
    'line_spacing_scale',
    'translation_gap_scale',
    'background_gap_scale',
    'horizontal_padding_scale',
  ]) {
    args.push(`--${key.replaceAll('_', '-')}`, String(options[key] === undefined ? 1 : options[key]));
  }
  args.push('--format', options.format || 'auto');
  if (options.background) args.push('--background', options.background);
  if (options.cover) args.push('--cover', options.cover);
  if (options.title) args.push('--title', options.title);
  if (options.no_embedded_cover) args.push('--no-embedded-cover');
  if (options.no_audio) args.push('--no-audio');
  if (options.progress_events) args.push('--progress-events');
  const result = spawnSync(binary(), args, {
    encoding: 'utf8',
    stdio: options.progress_events ? 'inherit' : 'pipe',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error((result.stderr || `goosy exited with status ${result.status}`).trim());
}

const native = nativeBinding();

module.exports = {
  binary,
  render: native ? native.render : renderCli,
  parseLyrics: native
    ? native.parseLyrics
    : () => { throw new Error('parseLyrics requires the compiled goosy-node native binding'); },
};
