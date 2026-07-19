#!/usr/bin/env node
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const runtimePath = path.resolve(here, "../src/bitneedle-ordinal-runtime.js");
const inputPath = process.argv[2];

if (!inputPath) {
  console.error("usage: node ordinals/tools/compact-manifest.mjs <manifest.json>");
  process.exit(2);
}

vm.runInThisContext(readFileSync(runtimePath, "utf8"), { filename: runtimePath });

const manifest = JSON.parse(readFileSync(inputPath, "utf8"));
const compact = globalThis.BitneedleOrdinal.compactManifest(manifest);
process.stdout.write(`${JSON.stringify(compact)}\n`);
