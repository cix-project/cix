import { readFile, writeFile } from 'node:fs/promises';
import { PortableCix } from './cix-portable.mjs';
const [mode, wasmPath, inputPath, archivePath, cap] = process.argv.slice(2);
const cix = await PortableCix.instantiate(await readFile(wasmPath));
const input = new Uint8Array(await readFile(inputPath));
const result = mode === 'encode'
  ? cix.encode(input, Number(cap))
  : mode === 'decode' ? cix.decode(input, Number(cap)) : (() => { throw new Error('mode must be encode or decode'); })();
await writeFile(archivePath, result);
