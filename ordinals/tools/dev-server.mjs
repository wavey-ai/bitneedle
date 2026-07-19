#!/usr/bin/env node
import { createServer } from "node:http";
import { mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../..");
const runtimeEntry = path.resolve(here, "../src/bitneedle-ordinal-runtime.js");
const defaultManifestPath = path.resolve(here, "../examples/recursive-manifest.json");
const distDir = path.resolve(root, ".tmp-local/ordinals-dev");
const runtimeOut = path.join(distDir, "bitneedle-ordinal-runtime.js");
const assetCache = new Map();

function argValue(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] || fallback : fallback;
}

const host = argValue("--host", process.env.HOST || "127.0.0.1");
const port = Number(argValue("--port", process.env.PORT || "5177"));
const manifestPath = path.resolve(argValue("--manifest", defaultManifestPath));

function send(res, status, body, contentType = "text/plain; charset=utf-8") {
  res.writeHead(status, {
    "content-type": contentType,
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
  });
  res.end(body);
}

function htmlEscape(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function devPage() {
  const manifest = readFileSync(manifestPath, "utf8");
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Bitneedle Ordinals Dev</title>
    <style>
      :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
      body { margin: 0; min-height: 100vh; display: grid; grid-template-columns: minmax(340px, 460px) 1fr; background: #111; color: #f4f4f4; }
      aside { box-sizing: border-box; padding: 16px; border-right: 1px solid #333; display: grid; grid-template-rows: auto 1fr auto auto; gap: 12px; }
      main { display: grid; place-items: center; padding: 24px; }
      h1 { margin: 0; font-size: 18px; }
      textarea { box-sizing: border-box; width: 100%; min-height: 360px; resize: vertical; font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; color: #eee; background: #050505; border: 1px solid #444; border-radius: 8px; padding: 10px; }
      button, a { color: inherit; }
      button { border: 1px solid #555; background: #222; border-radius: 8px; padding: 9px 12px; cursor: pointer; }
      button:disabled { opacity: 0.45; cursor: not-allowed; }
      .buttons { display: flex; flex-wrap: wrap; gap: 8px; }
      pre { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0; font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; color: #cfcfcf; }
      canvas { width: min(576px, 90vw); height: min(576px, 90vw); background: #fff; box-shadow: 0 8px 40px rgba(0,0,0,.35); }
      @media (max-width: 880px) { body { grid-template-columns: 1fr; } aside { border-right: 0; border-bottom: 1px solid #333; } }
    </style>
  </head>
  <body>
    <aside>
      <h1>Bitneedle Ordinals Dev</h1>
      <textarea id="manifest" spellcheck="false">${htmlEscape(manifest.trim())}</textarea>
      <div class="buttons">
        <button id="render" type="button">Render</button>
        <button id="download" type="button" disabled>Download PNG</button>
        <button id="reset" type="button">Reset JSON</button>
      </div>
      <pre id="status">Ready.</pre>
    </aside>
    <main>
      <canvas id="bitneedle-record" width="576" height="576"></canvas>
    </main>
    <script src="/dist/bitneedle-ordinal-runtime.js"></script>
    <script>
      const initialManifest = document.getElementById("manifest").value;
      const canvas = document.getElementById("bitneedle-record");
      const status = document.getElementById("status");
      const download = document.getElementById("download");
      let lastResult = null;

      async function render() {
        download.disabled = true;
        status.textContent = "Rendering…";
        const manifest = JSON.parse(document.getElementById("manifest").value);
        lastResult = await BitneedleOrdinal.renderRecord(manifest, { canvas, baseUrl: location.origin });
        download.disabled = false;
        status.textContent = JSON.stringify(lastResult.stats, null, 2);
      }

      document.getElementById("render").addEventListener("click", () => {
        render().catch((error) => {
          status.textContent = error && error.stack || String(error);
        });
      });
      document.getElementById("reset").addEventListener("click", () => {
        document.getElementById("manifest").value = initialManifest;
      });
      download.addEventListener("click", () => {
        if (lastResult) BitneedleOrdinal.downloadPng(lastResult).catch((error) => {
          status.textContent = error && error.stack || String(error);
        });
      });
      render().catch((error) => {
        status.textContent = error && error.stack || String(error);
      });
    </script>
  </body>
</html>`;
}

function contentTypeFor(pathname) {
  if (pathname.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (pathname.endsWith(".json")) return "application/json; charset=utf-8";
  if (pathname.endsWith(".html")) return "text/html; charset=utf-8";
  if (pathname.endsWith(".png")) return "image/png";
  if (pathname.endsWith(".avif")) return "image/avif";
  return "application/octet-stream";
}

async function proxyContent(pathname, res) {
  if (!assetCache.has(pathname)) {
    const upstream = await fetch(`https://ordinals.com${pathname}`);
    if (!upstream.ok) {
      send(res, upstream.status, `Ordinals upstream failed: HTTP ${upstream.status}`);
      return;
    }
    assetCache.set(pathname, {
      contentType: upstream.headers.get("content-type") || "application/octet-stream",
      body: Buffer.from(await upstream.arrayBuffer()),
    });
  }
  const cached = assetCache.get(pathname);
  send(res, 200, cached.body, cached.contentType);
}

mkdirSync(distDir, { recursive: true });
const esbuildContext = await esbuild.context({
  entryPoints: [runtimeEntry],
  bundle: true,
  format: "iife",
  target: "es2022",
  sourcemap: "inline",
  outfile: runtimeOut,
  logLevel: "info",
});
await esbuildContext.rebuild();
await esbuildContext.watch();

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", `http://${host}:${port}`);
    if (url.pathname === "/" || url.pathname === "/index.html") {
      send(res, 200, devPage(), "text/html; charset=utf-8");
      return;
    }
    if (url.pathname === "/manifest.json") {
      send(res, 200, readFileSync(manifestPath), "application/json; charset=utf-8");
      return;
    }
    if (url.pathname === "/dist/bitneedle-ordinal-runtime.js") {
      send(res, 200, readFileSync(runtimeOut), contentTypeFor(url.pathname));
      return;
    }
    if (url.pathname.startsWith("/content/")) {
      await proxyContent(url.pathname, res);
      return;
    }
    send(res, 404, "Not found");
  } catch (error) {
    send(res, 500, error && error.stack || String(error));
  }
});

server.listen(port, host, () => {
  const actual = server.address();
  const origin = `http://${actual.address}:${actual.port}`;
  console.log(`Bitneedle Ordinals dev server`);
  console.log(`  ${origin}/`);
  console.log(`  manifest: ${path.relative(root, manifestPath)}`);
  console.log(`  runtime:  ${path.relative(root, runtimeOut)}`);
});

async function shutdown() {
  server.close();
  await esbuildContext.dispose();
}

process.on("SIGINT", () => {
  shutdown().finally(() => process.exit(0));
});
process.on("SIGTERM", () => {
  shutdown().finally(() => process.exit(0));
});
