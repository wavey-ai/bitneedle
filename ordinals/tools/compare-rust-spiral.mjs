#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../..");
const runtimePath = path.resolve(here, "../src/bitneedle-ordinal-runtime.js");

const cases = [
  { width: 576, height: 576, bValue: 0.25, profile: "single45" },
  { width: 576, height: 576, bValue: 0.25, profile: "lp" },
  { width: 576, height: 576, bValue: 0.5, profile: "single45" },
];

vm.runInThisContext(readFileSync(runtimePath, "utf8"), { filename: runtimePath });

function sha256Indices(indices) {
  const hash = createHash("sha256");
  const buffer = Buffer.allocUnsafe(4);
  for (const index of indices) {
    buffer.writeUInt32BE(index >>> 0, 0);
    hash.update(buffer);
  }
  return hash.digest("hex");
}

function rustSummary(testCase) {
  const result = spawnSync(
    "cargo",
    [
      "run",
      "-q",
      "-p",
      "record-core",
      "--example",
      "spiral_summary",
      "--",
      String(testCase.width),
      String(testCase.height),
      String(testCase.bValue),
      testCase.profile,
    ],
    {
      cwd: root,
      encoding: "utf8",
      env: { ...process.env, EMSDK_QUIET: "1" },
    },
  );

  if (result.status !== 0) {
    throw new Error(`record-core spiral_summary failed:\n${result.stderr || result.stdout}`);
  }

  return JSON.parse(result.stdout.trim());
}

function compareCase(testCase) {
  const rust = rustSummary(testCase);
  const geometry = globalThis.BitneedleOrdinal.PROFILE_GEOMETRY[testCase.profile];
  const indices = globalThis.BitneedleOrdinal.buildPayloadSpiralIndices({
    width: testCase.width,
    height: testCase.height,
    bValue: testCase.bValue,
    geometry,
  });
  const first = indices.slice(0, 32);
  const last = indices.slice(-32);
  const js = {
    addressablePixelCount: indices.length,
    indexSha256: sha256Indices(indices),
    first,
    last,
  };
  const failures = [];
  if (js.addressablePixelCount !== rust.addressablePixelCount) {
    failures.push(`count ${js.addressablePixelCount} !== ${rust.addressablePixelCount}`);
  }
  if (js.indexSha256 !== rust.indexSha256) {
    failures.push(`sha256 ${js.indexSha256} !== ${rust.indexSha256}`);
  }
  if (JSON.stringify(js.first) !== JSON.stringify(rust.first)) {
    failures.push("first indices differ");
  }
  if (JSON.stringify(js.last) !== JSON.stringify(rust.last)) {
    failures.push("last indices differ");
  }
  return { ...testCase, ok: failures.length === 0, failures, js, rust };
}

const results = cases.map(compareCase);
const ok = results.every((result) => result.ok);
process.stdout.write(`${JSON.stringify({ ok, results })}\n`);
if (!ok) process.exit(1);
