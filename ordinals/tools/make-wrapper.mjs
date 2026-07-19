#!/usr/bin/env node
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const runtimePath = path.resolve(here, "../src/bitneedle-ordinal-runtime.js");

function usage() {
  console.error("usage: node ordinals/tools/make-wrapper.mjs [--runtime <src>] <manifest.json>");
  process.exit(2);
}

function normalizeContentPath(value) {
  const raw = String(value || "").trim();
  if (/^[0-9a-f]{64}i\d+$/i.test(raw)) return `/content/${raw}`;
  return raw;
}

function escapeAttribute(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;");
}

function scriptJson(value) {
  return JSON.stringify(value).replace(/</g, "\\u003c");
}

let runtimeSrc = "../src/bitneedle-ordinal-runtime.js";
let inputPath = "";

for (let i = 2; i < process.argv.length; i += 1) {
  const arg = process.argv[i];
  if (arg === "--runtime" || arg === "-r") {
    runtimeSrc = process.argv[++i] || usage();
  } else if (!inputPath) {
    inputPath = arg;
  } else {
    usage();
  }
}

if (!inputPath) usage();

vm.runInThisContext(readFileSync(runtimePath, "utf8"), { filename: runtimePath });

const manifest = JSON.parse(readFileSync(inputPath, "utf8"));
const compact = globalThis.BitneedleOrdinal.compactManifest(manifest);
const normalized = globalThis.BitneedleOrdinal.normalizeManifest(compact);
const src = escapeAttribute(normalizeContentPath(runtimeSrc));
const json = scriptJson(compact);

process.stdout.write(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>html,body{margin:0;min-height:100%;display:grid;place-items:center;background:#fff}#bitneedle-record{width:min(${normalized.width}px,100vw);height:min(${normalized.height}px,100vw)}#bitneedle-download{position:fixed;right:12px;bottom:12px}</style></head><body><canvas id="bitneedle-record" width="${normalized.width}" height="${normalized.height}"></canvas><button id="bitneedle-download" type="button" disabled>Download PNG</button><script src="${src}"></script><script id="bitneedle-ordinal-record" type="application/json">${json}</script><script>BitneedleOrdinal.bootstrap().catch(e=>{document.body.textContent=e&&e.stack||String(e)})</script></body></html>\n`);
