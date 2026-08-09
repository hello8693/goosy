const fs = require('node:fs');
const path = require('node:path');

const root = __dirname;
const extension = process.platform === 'win32' ? '.dll' : process.platform === 'darwin' ? '.dylib' : '.so';
const source = path.join(root, 'target', 'release', `libgoosy_node${extension}`);
const destination = path.join(root, 'goosy-node', `goosy_node.${process.platform}-${process.arch}.node`);
if (!fs.existsSync(source)) {
  throw new Error(`Native binding not found: ${source}`);
}
fs.copyFileSync(source, destination);
console.log(`Wrote ${destination}`);
