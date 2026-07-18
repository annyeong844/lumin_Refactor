'use strict';

const path = require('node:path');

if (process.argv.length !== 3) {
  throw new Error('usage: node extract_compiler_options.cjs <exact-typescript.js>');
}

const typescriptPath = path.resolve(process.argv[2]);
const ts = require(typescriptPath);
if (!Array.isArray(ts.optionDeclarations)) {
  throw new Error('typescript optionDeclarations is unavailable');
}

const normalized = new Map();
for (const option of ts.optionDeclarations) {
  if (option.isCommandLineOnly === true) continue;
  if (typeof option.name !== 'string' || option.name.length === 0) {
    throw new Error('invalid compiler option name');
  }

  let shape;
  if (typeof option.type === 'string') {
    shape = option.type;
  } else if (option.type instanceof Map) {
    shape = 'enum';
  } else {
    throw new Error(`unsupported compiler option shape for ${option.name}`);
  }

  if (!['boolean', 'enum', 'list', 'number', 'object', 'string'].includes(shape)) {
    throw new Error(`unknown normalized compiler option shape ${shape}`);
  }
  const previous = normalized.get(option.name);
  if (previous !== undefined && previous !== shape) {
    throw new Error(`conflicting compiler option shape for ${option.name}`);
  }
  normalized.set(option.name, shape);
}

const names = [...normalized.keys()].sort((left, right) =>
  Buffer.compare(Buffer.from(left, 'utf8'), Buffer.from(right, 'utf8'))
);
for (const name of names) {
  process.stdout.write(`${name}\t${normalized.get(name)}\n`);
}
