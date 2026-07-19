(function attachBitneedleOrdinal(globalScope) {
  "use strict";

  const DEFAULT_SIZE = 576;
  const DEFAULT_START_ANGLE = Math.PI / 2;
  const DEFAULT_BG = "#fafafa";
  const DEFAULT_RECORD = "#000000";
  const DEFAULT_LABEL = "#fafafa";
  const DEFAULT_TEXT = "#000000";
  const DEFAULT_SPIRAL_MODE = "record-core";
  const DEFAULT_LABEL_STYLE = "master";

  // Canonical 576px Bitneedle carrier geometry, copied from record-core's
  // single45/lp profile calculations. The runtime stays small by hard-coding
  // these two public carrier profiles and allowing manifest-level overrides.
  const PROFILE_GEOMETRY = Object.freeze({
    single45: Object.freeze({
      recordProfile: "single45",
      spindleHoleRadius: 12,
      dinkRadius: 63,
      labelRadius: 151,
      payloadInnerRadius: 169,
      payloadOuterRadius: 280,
      outerRadius: 287,
      outerRimThickness: 4,
      leadInBandThickness: 6,
    }),
    lp: Object.freeze({
      recordProfile: "lp",
      spindleHoleRadius: 7,
      dinkRadius: null,
      labelRadius: 95,
      payloadInnerRadius: 109,
      payloadOuterRadius: 280,
      outerRadius: 287,
      outerRimThickness: 4,
      leadInBandThickness: 6,
    }),
  });

  function clamp(value, min, max, fallback = min) {
    const number = Number(value);
    return Number.isFinite(number) ? Math.max(min, Math.min(max, number)) : fallback;
  }

  function int(value, fallback = 0) {
    const number = Number(value);
    return Number.isFinite(number) ? Math.round(number) : fallback;
  }

  function jsRound(value) {
    return Math.floor(value + 0.5);
  }

  function pick(...values) {
    for (const value of values) {
      if (value !== undefined && value !== null && value !== "") return value;
    }
    return undefined;
  }

  function hasOwn(object, key) {
    return Object.prototype.hasOwnProperty.call(Object(object), key);
  }

  function normalizeProfileName(value) {
    const name = String(value || "single45").trim();
    if (name === "single45" || name === "45" || name === "single") return "single45";
    if (name === "lp" || name === "LP" || name === "album") return "lp";
    throw new Error(`Unknown Bitneedle record profile: ${value}`);
  }

  function profileGeometry(profile, overrides = {}) {
    const name = normalizeProfileName(profile);
    const base = PROFILE_GEOMETRY[name];
    const geometry = { ...base };
    for (const [key, value] of Object.entries(overrides || {})) {
      if (value !== undefined && value !== null && value !== "") {
        geometry[key] = typeof base[key] === "number" ? int(value, base[key]) : value;
      }
    }
    geometry.recordProfile = name;
    return geometry;
  }

  function normalizeContentPath(value) {
    if (value == null || value === "") return "";
    const raw = String(value).trim();
    if (!raw) return "";
    if (/^(?:https?:|data:|blob:|ipfs:)/i.test(raw)) return raw;
    if (raw.startsWith("/")) return raw;
    if (/^[0-9a-f]{64}i\d+$/i.test(raw)) return `/content/${raw}`;
    return raw;
  }

  function ordinalPath(path, baseUrl = "") {
    const normalized = normalizeContentPath(path);
    if (!normalized) return "";
    if (/^(?:https?:|data:|blob:|ipfs:)/i.test(normalized)) return normalized;
    if (baseUrl && normalized.startsWith("/content/")) {
      return `${String(baseUrl).replace(/\/+$/, "")}${normalized}`;
    }
    return normalized;
  }

  function normalizeSpiralMode(value) {
    const text = String(value || DEFAULT_SPIRAL_MODE).trim().toLowerCase();
    if (text === "record-core" || text === "rust" || text === "arch" || text === "archimedean") return "record-core";
    if (text === "legacy" || text === "legacy-log" || text === "log" || text === "old") {
      throw new Error("The old logarithmic ordinal spiral is not supported. Use record-core.");
    }
    throw new Error(`Unknown Bitneedle ordinal spiral mode: ${value}`);
  }

  function normalizeLabelStyle(value) {
    const text = String(value || DEFAULT_LABEL_STYLE).trim().toLowerCase();
    if (text === "master" || text === "demo" || text === "original") return "master";
    if (text === "clean" || text === "press" || text === "modern") return "clean";
    throw new Error(`Unknown Bitneedle ordinal label style: ${value}`);
  }

  function loadImage(src, { baseUrl = "", mirrorFallback = true } = {}) {
    const url = ordinalPath(src, baseUrl);
    if (!url) return Promise.resolve(null);
    return new Promise((resolve, reject) => {
      const image = new Image();
      image.crossOrigin = "anonymous";
      image.onload = () => resolve(image);
      image.onerror = () => {
        if (
          mirrorFallback &&
          url.startsWith("https://ordinals.com/content/")
        ) {
          image.onerror = () => reject(new Error(`Failed to load image: ${url}`));
          image.src = url.replace("https://ordinals.com", "https://ord-mirror.magiceden.dev");
          return;
        }
        reject(new Error(`Failed to load image: ${url}`));
      };
      image.src = url;
    });
  }

  function makeCanvas(width = DEFAULT_SIZE, height = DEFAULT_SIZE) {
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, int(width, DEFAULT_SIZE));
    canvas.height = Math.max(1, int(height, DEFAULT_SIZE));
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("Canvas 2D is unavailable.");
    return { canvas, context };
  }

  function imageToImageData(image, {
    cropScanlines = false,
    width = image?.naturalWidth || image?.width,
    height = image?.naturalHeight || image?.height,
  } = {}) {
    if (!image) throw new Error("Cannot read pixels from a missing image.");
    const sourceHeight = Math.max(1, int(height, image.height || 1));
    const sourceWidth = Math.max(1, int(width, image.width || 1));
    const cropTop = cropScanlines && sourceHeight > 4 ? 2 : 0;
    const cropHeight = cropScanlines && sourceHeight > 4 ? sourceHeight - 4 : sourceHeight;
    const { canvas, context } = makeCanvas(sourceWidth, cropHeight);
    context.drawImage(image, 0, cropTop, sourceWidth, cropHeight, 0, 0, sourceWidth, cropHeight);
    return context.getImageData(0, 0, canvas.width, canvas.height);
  }

  function readRgbBlockPixels(imageData, { byteLength, pixelCount, ignoreTransparent = true } = {}) {
    const data = imageData.data;
    const declaredPixelCount = pixelCount || (byteLength ? Math.ceil(Number(byteLength) / 3) : 0);
    const maxPixels = Math.floor(data.length / 4);
    const count = declaredPixelCount > 0 ? Math.min(maxPixels, int(declaredPixelCount)) : maxPixels;
    const out = new Uint8ClampedArray(count * 4);
    let written = 0;
    for (let i = 0; i < maxPixels && written < count; i += 1) {
      const source = i * 4;
      const alpha = data[source + 3];
      if (!declaredPixelCount && ignoreTransparent && alpha === 0) continue;
      const dest = written * 4;
      out[dest] = data[source];
      out[dest + 1] = data[source + 1];
      out[dest + 2] = data[source + 2];
      out[dest + 3] = alpha || 255;
      written += 1;
    }
    return written === count ? out : out.slice(0, written * 4);
  }

  function traceRecordSpiral({
    width = DEFAULT_SIZE,
    height = DEFAULT_SIZE,
    bValue,
    pitch,
    startAngle = DEFAULT_START_ANGLE,
    pixelGap = 1,
    clockwise = true,
    outerRadius,
    innerRadius = 0,
  }) {
    const centerX = width / 2;
    const centerY = height / 2;
    const recordRadius = Math.min(width, height) / 2;
    const resolvedPitch = Number(pitch || bValue);
    if (!(resolvedPitch > 0)) throw new Error("A positive spiral pitch is required.");
    const boundedOuter = Math.min(Number(outerRadius), recordRadius - 1);
    const boundedInner = Math.max(0, Number(innerRadius) || 0);
    const occupied = new Uint8Array(width * height);
    const ordered = [];
    let sweptTheta = 0;
    let angle = startAngle;
    let radius = boundedOuter;
    while (radius >= boundedInner) {
      const x = jsRound(centerX + radius * Math.cos(angle));
      const y = jsRound(centerY - radius * Math.sin(angle));
      if (x >= 0 && x < width && y >= 0 && y < height) {
        const pixelIndex = y * width + x;
        if (occupied[pixelIndex] === 0) {
          occupied[pixelIndex] = 1;
          ordered.push(pixelIndex);
        }
      }
      const thetaStep = pixelGap / Math.sqrt(radius * radius + resolvedPitch * resolvedPitch);
      sweptTheta += thetaStep;
      angle = startAngle + (clockwise ? -sweptTheta : sweptTheta);
      radius = boundedOuter - resolvedPitch * sweptTheta;
    }
    return { occupied, ordered, centerX, centerY };
  }

  function buildPayloadSpiralIndices({ width, height, bValue, geometry }) {
    const traced = traceRecordSpiral({
      width,
      height,
      bValue,
      outerRadius: geometry.payloadOuterRadius,
      innerRadius: 0,
    });
    const indices = [];
    for (const pixelIndex of traced.ordered) {
      if (traced.occupied[pixelIndex] === 0) continue;
      const x = pixelIndex % width;
      const y = Math.floor(pixelIndex / width);
      const dx = x - traced.centerX;
      const dy = y - traced.centerY;
      const distance = Math.sqrt(dx * dx + dy * dy);
      if (distance > geometry.payloadInnerRadius && distance < geometry.payloadOuterRadius) {
        indices.push(pixelIndex);
      }
    }
    return indices;
  }

  function countAnnulusPixels(width, height, geometry) {
    const centerX = width / 2;
    const centerY = height / 2;
    let count = 0;
    for (let y = 0; y < height; y += 1) {
      for (let x = 0; x < width; x += 1) {
        const dx = x - centerX;
        const dy = y - centerY;
        const distance = Math.sqrt(dx * dx + dy * dy);
        if (distance > geometry.payloadInnerRadius && distance < geometry.payloadOuterRadius) count += 1;
      }
    }
    return count;
  }

  function estimateBValue(pixelCount, geometry) {
    const inner = geometry.payloadInnerRadius;
    const outer = geometry.payloadOuterRadius;
    const annulusArea = Math.max(1, outer * outer - inner * inner);
    return Math.max(1e-7, annulusArea / (2 * Math.max(1, pixelCount)));
  }

  function fitBValue(pixelCount, {
    width = DEFAULT_SIZE,
    height = DEFAULT_SIZE,
    geometry,
    initialBValue,
    iterations = 14,
  }) {
    const target = Math.max(1, int(pixelCount, 1));
    const capacity = countAnnulusPixels(width, height, geometry);
    let lowB = Math.max(1e-7, Number(initialBValue) || estimateBValue(target, geometry));
    let highB = lowB;
    let low = buildPayloadSpiralIndices({ width, height, bValue: lowB, geometry }).length;
    let high = low;

    while (low < target && lowB > 1e-7) {
      highB = lowB;
      high = low;
      lowB = Math.max(1e-7, lowB / 1.35);
      low = buildPayloadSpiralIndices({ width, height, bValue: lowB, geometry }).length;
      if (lowB <= 1e-7) break;
    }
    while (high > target && highB < 10) {
      lowB = highB;
      low = high;
      highB *= 1.35;
      high = buildPayloadSpiralIndices({ width, height, bValue: highB, geometry }).length;
      if (highB > 10) break;
    }

    let best = { bValue: lowB, count: low, distance: Math.abs(target - low) };
    const consider = (bValue, count) => {
      const distance = Math.abs(target - count);
      if (
        distance < best.distance ||
        (distance === best.distance && count >= target && best.count < target)
      ) {
        best = { bValue, count, distance };
      }
    };
    consider(highB, high);
    for (let i = 0; i < iterations; i += 1) {
      const midB = (lowB + highB) / 2;
      const mid = buildPayloadSpiralIndices({ width, height, bValue: midB, geometry }).length;
      consider(midB, mid);
      if (mid >= target) lowB = midB;
      else highB = midB;
    }

    return { ...best, capacity };
  }

  function drawCenteredImage(context, image, x, y, width, height, { cover = true } = {}) {
    if (!image) return;
    const sourceWidth = image.naturalWidth || image.width;
    const sourceHeight = image.naturalHeight || image.height;
    const targetRatio = width / height;
    const sourceRatio = sourceWidth / sourceHeight;
    let sx = 0;
    let sy = 0;
    let sw = sourceWidth;
    let sh = sourceHeight;
    if (cover ? sourceRatio > targetRatio : sourceRatio < targetRatio) {
      sw = sourceHeight * targetRatio;
      sx = (sourceWidth - sw) / 2;
    } else {
      sh = sourceWidth / targetRatio;
      sy = (sourceHeight - sh) / 2;
    }
    context.drawImage(image, sx, sy, sw, sh, x, y, width, height);
  }

  function drawCircle(context, x, y, radius, fill, stroke, lineWidth = 1) {
    context.beginPath();
    context.arc(x, y, Math.max(0, radius), 0, Math.PI * 2);
    if (fill) {
      context.fillStyle = fill;
      context.fill();
    }
    if (stroke) {
      context.strokeStyle = stroke;
      context.lineWidth = lineWidth;
      context.stroke();
    }
  }

  function drawCurvedText(context, text, {
    centerX,
    centerY,
    radius,
    color,
    font = "6pt sans-serif",
    startAngle = -Math.PI,
    endAngle = Math.PI,
  }) {
    const chars = Array.from(String(text || ""));
    if (!chars.length) return;
    context.save();
    context.fillStyle = color;
    context.font = font;
    context.textAlign = "center";
    context.textBaseline = "middle";
    const span = endAngle - startAngle;
    const step = span / chars.length;
    for (let i = 0; i < chars.length; i += 1) {
      const angle = startAngle + i * step;
      const x = centerX + radius * Math.cos(angle);
      const y = centerY + radius * Math.sin(angle);
      context.save();
      context.translate(x, y);
      context.rotate(angle + Math.PI / 2);
      context.fillText(chars[i], 0, 0);
      context.restore();
    }
    context.restore();
  }

  function drawLabel(context, manifest, loaded) {
    const { width, height, geometry, label } = manifest;
    const cx = width / 2;
    const cy = height / 2;
    const labelRadius = Number(label.radius || geometry.labelRadius);
    drawCircle(context, cx, cy, labelRadius + 10, label.borderColor || manifest.recordColor);
    drawCircle(context, cx, cy, labelRadius, label.background || manifest.labelColor);

    if (loaded.labelArtwork) {
      context.save();
      context.beginPath();
      context.arc(cx, cy, Math.max(1, labelRadius - 12), 0, Math.PI * 2);
      context.clip();
      context.globalAlpha = clamp(label.artworkOpacity, 0, 1, 0.3);
      drawCenteredImage(
        context,
        loaded.labelArtwork,
        cx - labelRadius,
        cy - labelRadius,
        labelRadius * 2,
        labelRadius * 2,
      );
      context.restore();
    }

    context.save();
    context.fillStyle = label.textColor || manifest.textColor;
    context.textAlign = "center";
    context.textBaseline = "middle";

    if (label.side) {
      context.fillStyle = label.sideColor || "#ff2c2c";
      context.font = "bold 80px sans-serif";
      context.fillText(String(label.side).toUpperCase(), cx, cy + 4);
      context.fillStyle = label.textColor || manifest.textColor;
      context.font = "bold 8pt sans-serif";
      context.textAlign = "start";
      context.textBaseline = "alphabetic";
      context.fillText("ADVANCE", cx - 89, cy - 80);
      context.fillText("PRESSING", cx - 90, cy - 70);
      context.textAlign = "center";
      context.textBaseline = "middle";
    }

    context.font = label.titleFont || "bold 14pt sans-serif";
    if (label.title) context.fillText(String(label.title).toUpperCase(), cx, cy + 70);
    context.font = label.subtitleFont || "6pt sans-serif";
    if (label.subtitle) context.fillText(label.subtitle, cx, cy + 85);
    context.font = label.artistFont || "bold 15pt sans-serif";
    if (label.artist) context.fillText(String(label.artist).toUpperCase(), cx, cy + 108);

    context.font = "bold 7pt sans-serif";
    context.fillText(label.masterText || "BTC MASTER", cx - 90, cy - 20);
    context.font = "bold 9pt arial";
    context.fillText(label.stereoText || "STEREO", cx - 96, cy - 6);
    context.font = "bold 7pt sans-serif";
    context.fillText(label.codecText || "ENCODEC 48", cx - 91, cy + 8);
    context.font = "7pt sans-serif";
    if (label.duration) context.fillText(`Time: ${label.duration}`, cx - 98, cy + 26);

    if (label.rightText) {
      context.textAlign = "right";
      context.font = "6pt sans-serif";
      String(label.rightText).split("\n").forEach((line, index) => {
        context.fillText(line, cx + 120, cy - 20 + 8 * index);
      });
    }
    if (label.leftText) {
      context.textAlign = "right";
      context.font = "6pt sans-serif";
      String(label.leftText).split("\n").forEach((line, index) => {
        context.fillText(line, cx - 94, cy + 42 + 8 * index);
      });
    }
    if (label.publisherYear) {
      context.textAlign = "left";
      context.font = "7pt sans-serif";
      context.fillText("℗", cx + 90, cy + 20);
      context.font = "6pt sans-serif";
      context.fillText(String(label.publisherYear), cx + 108, cy + 19);
    }
    context.restore();

    const runout = pick(label.runoutText, label.ordinalText);
    if (runout) {
      drawCurvedText(context, runout, {
        centerX: cx,
        centerY: cy,
        radius: Math.max(1, labelRadius - 10),
        color: label.textColor || manifest.textColor,
      });
    }

    drawCircle(context, cx, cy, geometry.spindleHoleRadius + 4, "#ffffff", "#dddddd", 2);
  }

  function drawMasterLabel(context, manifest, loaded) {
    const { width, height, geometry, label } = manifest;
    const cx = width / 2;
    const cy = height / 2;
    const labelRadius = Number(label.radius || geometry.labelRadius);
    const textColor = label.textColor || manifest.textColor;

    context.fillStyle = manifest.recordColor;
    context.beginPath();
    context.arc(cx, cy, labelRadius + 10, 0, Math.PI * 2);
    context.fill();

    context.fillStyle = label.background || manifest.labelColor;
    context.beginPath();
    context.arc(cx, cy, labelRadius, 0, Math.PI * 2);
    context.fill();

    context.textAlign = "center";
    context.textBaseline = "middle";

    if (label.side) {
      context.fillStyle = "#a9b0b4";
      context.font = "bold 8pt sans-serif";
      context.textAlign = "start";
      context.textBaseline = "alphabetic";
      context.fillText("ADVANCE", cx - 89, cy - 80);
      context.fillText("PRESSING", cx - 90, cy - 70);
      context.textAlign = "center";
      context.textBaseline = "middle";
      context.fillStyle = label.sideColor || "#ff2c2c";
      context.font = "bold 80px sans-serif";
      context.fillText(String(label.side).toUpperCase(), cx, cy + 4);
    }

    context.fillStyle = textColor;
    context.font = "bold 14pt sans-serif";
    if (label.title) context.fillText(String(label.title).toUpperCase(), cx, cy + 70);
    context.font = "6pt sans-serif";
    if (label.subtitle) context.fillText(label.subtitle, cx, cy + 85);
    context.font = "bold 15pt sans-serif";
    if (label.artist) context.fillText(String(label.artist).toUpperCase(), cx, cy + 108);

    context.font = "bold 7pt sans-serif";
    context.fillText(label.masterText || "BTC MASTER", cx - 90, cy - 20);
    context.font = "bold 9pt arial";
    context.fillText(label.stereoText || "STEREO", cx - 96, cy - 6);
    context.font = "bold 7pt sans-serif";
    context.fillText(label.codecText || "ENCODEC 48", cx - 91, cy + 8);
    context.font = "7pt sans-serif";
    if (label.duration) context.fillText(`Time: ${label.duration}`, cx - 98, cy + 26);
    if (label.publisherYear) {
      context.font = "12pt sans-serif";
      context.fillText("℗ ", cx + 90, cy + 20);
      context.font = "7pt sans-serif";
      context.fillText(String(label.publisherYear), cx + 108, cy + 19);
    }

    context.font = "6pt sans-serif";
    if (label.leftText) context.fillText(label.leftText, cx - 94, cy + 42);
    if (label.minedBy) {
      context.textAlign = "right";
      ["Mined by", label.minedBy].forEach((line, index) => {
        context.fillText(line, cx + 110, cy - 40 + 8 * index);
      });
    }
    if (label.rightText) {
      context.textAlign = "right";
      String(label.rightText).split("\n").forEach((line, index) => {
        context.fillText(line, cx + 120, cy - 20 + 8 * index);
      });
    }

    context.textAlign = "center";
    const runout = labelRunoutText(label);
    if (runout) {
      drawCurvedText(context, runout, {
        centerX: cx,
        centerY: cy,
        radius: Math.max(1, labelRadius - 10),
        color: textColor,
      });
    }

    context.beginPath();
    context.arc(cx, cy, 11, 0, Math.PI * 2);
    context.fill();

    if (loaded.artwork) {
      const scale = 0.8 + 0.2 * clamp(label.artworkOpacity, 0, 1, 0.4);
      const targetWidth = loaded.artwork.width * scale;
      const targetHeight = loaded.artwork.height * scale;
      context.drawImage(
        loaded.artwork,
        cx + 30 - targetWidth / 2,
        cy + 40 - targetHeight / 2,
        targetWidth,
        targetHeight,
      );
    }

    if (loaded.labelArtwork) {
      context.save();
      context.globalAlpha = 0.4;
      context.globalCompositeOperation = "color-dodge";
      context.drawImage(
        loaded.labelArtwork,
        width / 2 - loaded.labelArtwork.width / 2,
        height / 2 - loaded.labelArtwork.height / 2,
      );
      context.restore();
    }

    context.beginPath();
    context.arc(cx, cy, 11, 0, Math.PI * 2);
    context.fillStyle = "#fff";
    context.fill();
    context.strokeStyle = "#ddd";
    context.lineWidth = 2;
    context.stroke();
  }

  function labelRunoutText(label) {
    const direct = pick(label.runoutText, label.ordinalText);
    if (direct) return direct;
    const ordinal = pick(label.ordinalNumber, label.ordinal);
    const satoshi = label.satoshi;
    const block = label.block;
    if (ordinal || satoshi || block) {
      return `#${ordinal || ""} • Satoshi ${satoshi || ""} • Block ${block || ""} • `;
    }
    return "";
  }

  function parseManifest(input) {
    if (typeof input !== "string") return { ...(input || {}) };
    try {
      return JSON.parse(input);
    } catch (error) {
      throw new Error(`Invalid Bitneedle ordinal JSON: ${error.message}`);
    }
  }

  function validationIssues(manifest) {
    const issues = [];
    const add = (path, message) => issues.push({ path, message });
    const geometry = manifest.geometry || {};
    const radiusLimit = Math.min(manifest.width, manifest.height) / 2;

    if (!manifest.assets?.rgbBlock) add("assets.rgbBlock", "RGB block image path is required.");
    if (!(manifest.width > 0 && manifest.width <= 4096)) add("width", "Width must be between 1 and 4096.");
    if (!(manifest.height > 0 && manifest.height <= 4096)) add("height", "Height must be between 1 and 4096.");
    if (manifest.bValue !== undefined && !(Number(manifest.bValue) > 0)) add("bValue", "Spiral b-value must be positive.");
    if (manifest.payload?.byteLength !== undefined && !(Number(manifest.payload.byteLength) > 0)) add("payload.byteLength", "Byte length must be positive.");
    if (manifest.payload?.pixelCount !== undefined && !(Number(manifest.payload.pixelCount) > 0)) add("payload.pixelCount", "Pixel count must be positive.");
    if (!(Number(geometry.outerRadius) > 0 && Number(geometry.outerRadius) <= radiusLimit)) add("geometry.outerRadius", "Outer radius must fit inside the canvas.");
    if (!(Number(geometry.payloadInnerRadius) >= 0)) add("geometry.payloadInnerRadius", "Payload inner radius must be non-negative.");
    if (!(Number(geometry.payloadOuterRadius) > Number(geometry.payloadInnerRadius))) add("geometry.payloadOuterRadius", "Payload outer radius must exceed payload inner radius.");
    if (!(Number(geometry.payloadOuterRadius) <= Number(geometry.outerRadius))) add("geometry.payloadOuterRadius", "Payload outer radius must not exceed outer radius.");
    if (!(Number(geometry.labelRadius) >= 0 && Number(geometry.labelRadius) < Number(geometry.payloadInnerRadius))) add("geometry.labelRadius", "Label radius must be smaller than payload inner radius.");
    if (!(Number(geometry.spindleHoleRadius) >= 0 && Number(geometry.spindleHoleRadius) < Number(geometry.labelRadius))) add("geometry.spindleHoleRadius", "Spindle radius must be smaller than label radius.");

    return issues;
  }

  function validateManifest(input, { throwOnError = false } = {}) {
    const manifest = input?.assets && input?.geometry
      ? input
      : normalizeManifest(input, { validate: false });
    const issues = validationIssues(manifest);
    if (throwOnError && issues.length) {
      const summary = issues.map((issue) => `${issue.path}: ${issue.message}`).join("; ");
      throw new Error(`Invalid Bitneedle ordinal manifest: ${summary}`);
    }
    return { ok: issues.length === 0, issues, manifest };
  }

  function geometryOverrides(manifest) {
    const base = PROFILE_GEOMETRY[manifest.profile];
    const overrides = {};
    for (const [key, value] of Object.entries(manifest.geometry || {})) {
      if (key !== "recordProfile" && value !== base?.[key]) overrides[key] = value;
    }
    return overrides;
  }

  function hasKeys(object) {
    return !!object && Object.keys(object).length > 0;
  }

  function normalizeManifest(input, { validate = true } = {}) {
    const raw = parseManifest(input);
    const press = raw.press || raw.pressJson || raw.j || {};
    const paths = raw.paths || raw.assets || raw.ordinalPaths || press.ordinalPaths || press.paths || {};
    const record = raw.record || press.record || {};
    const payload = raw.payload || press.payload || {};
    const render = raw.render || press.render || {};
    const labelInput = raw.label || press.label || {};
    const profile = normalizeProfileName(pick(record.profile, record.recordProfile, raw.profile, raw.p, press.recordProfile, press.record_profile));
    const spiralMode = normalizeSpiralMode(pick(render.spiralMode, render.map, raw.spiralMode, raw.sm, raw.m, DEFAULT_SPIRAL_MODE));
    const labelStyle = normalizeLabelStyle(pick(labelInput.style, render.labelStyle, raw.labelStyle, raw.ls, DEFAULT_LABEL_STYLE));
    const geometry = profileGeometry(profile, pick(raw.geometry, record.geometry, render.geometry, raw.g, {}));
    const width = int(pick(raw.width, raw.w, render.width, press.width), DEFAULT_SIZE);
    const height = int(pick(raw.height, raw.h, render.height, press.height), DEFAULT_SIZE);
    const compactLabel = Array.isArray(raw.l) ? raw.l : [];
    const label = {
      title: pick(labelInput.title, raw.title, press.title, press.headerTitle, compactLabel[0]),
      artist: pick(labelInput.artist, raw.artist, press.artist, press.headerArtist, compactLabel[1]),
      subtitle: pick(labelInput.subtitle, labelInput.credit, raw.subtitle, compactLabel[2]),
      side: pick(labelInput.side, raw.side, raw.s, compactLabel[3], "A"),
      duration: pick(labelInput.duration, raw.duration, press.durationText, compactLabel[4]),
      publisherYear: pick(labelInput.publisherYear, labelInput.year, raw.year, press.copyrightYear, compactLabel[5]),
      runoutText: pick(labelInput.runoutText, labelInput.ordinalText, raw.runoutText, raw.o, compactLabel[6]),
      rightText: pick(labelInput.rightText, raw.rightText, compactLabel[7]),
      leftText: pick(labelInput.leftText, raw.leftText, compactLabel[8]),
      ordinalNumber: pick(labelInput.ordinalNumber, labelInput.ordinal, raw.ordinalNumber, raw.on),
      satoshi: pick(labelInput.satoshi, raw.satoshi, raw.sat),
      block: pick(labelInput.block, raw.block, raw.bn),
      minedBy: pick(labelInput.minedBy, raw.minedBy, raw.mb),
      background: pick(labelInput.background, labelInput.bg, raw.labelBg, raw.lb, DEFAULT_LABEL),
      textColor: pick(labelInput.textColor, labelInput.fg, raw.textColor, raw.tc, DEFAULT_TEXT),
      sideColor: pick(labelInput.sideColor, raw.sideColor, raw.sc, "#ff2c2c"),
      artworkOpacity: pick(labelInput.artworkOpacity, raw.labelArtworkOpacity, raw.lao, 0.4),
      radius: pick(labelInput.radius, raw.labelRadius, raw.lr, geometry.labelRadius),
    };

    const manifest = {
      version: pick(raw.version, raw.v, 1),
      width,
      height,
      profile,
      spiralMode,
      labelStyle,
      geometry,
      backgroundColor: pick(render.backgroundColor, raw.backgroundColor, raw.bg, DEFAULT_BG),
      recordColor: pick(render.recordColor, raw.recordColor, raw.fg, raw.rc, DEFAULT_RECORD),
      labelColor: pick(render.labelColor, raw.labelColor, raw.lb, DEFAULT_LABEL),
      textColor: pick(render.textColor, raw.textColor, raw.tc, DEFAULT_TEXT),
      artworkOpacity: pick(render.artworkOpacity, raw.artworkOpacity, raw.ao, 0.45),
      overlayOpacity: pick(render.overlayOpacity, raw.overlayOpacity, raw.oo, 0.35),
      bValue: pick(render.bValue, raw.bValue, raw.b, press.bValue, press.spiral?.bValue),
      fit: pick(render.fit, raw.fit, !hasOwn(raw, "b") && !hasOwn(raw, "bValue")),
      cropScanlines: pick(payload.cropScanlines, raw.cropScanlines, raw.cs, false),
      payload: {
        byteLength: pick(payload.byteLength, payload.streamByteLength, raw.byteLength, raw.bl, press.streamByteLength),
        pixelCount: pick(payload.pixelCount, raw.pixelCount, raw.pc),
        ignoreTransparent: pick(payload.ignoreTransparent, raw.ignoreTransparent, true),
      },
      assets: {
        rgbBlock: normalizeContentPath(pick(paths.rgbBlock, paths.rgb, paths.track, raw.rgbBlock, raw.rgb, raw.t, raw.r)),
        artwork: normalizeContentPath(pick(paths.artwork, paths.cover, raw.artwork, raw.a)),
        labelArtwork: normalizeContentPath(pick(paths.labelArtwork, paths.label, paths.mark, raw.labelArtwork, raw.la)),
        overlay: normalizeContentPath(pick(paths.overlay, raw.overlay, raw.ov)),
      },
      label,
      sleeve: {
        enabled: !!pick(render.sleeve, raw.sleeve, raw.sl, false),
      },
    };
    if (validate) validateManifest(manifest, { throwOnError: true });
    return manifest;
  }

  function compactManifest(input) {
    const manifest = normalizeManifest(input);
    const compact = {
      v: manifest.version,
      p: manifest.profile,
      r: manifest.assets.rgbBlock,
    };
    if (manifest.spiralMode !== DEFAULT_SPIRAL_MODE) compact.sm = manifest.spiralMode;
    if (manifest.labelStyle !== DEFAULT_LABEL_STYLE) compact.ls = manifest.labelStyle;
    if (manifest.width !== DEFAULT_SIZE) compact.w = manifest.width;
    if (manifest.height !== DEFAULT_SIZE) compact.h = manifest.height;
    if (manifest.assets.artwork) compact.a = manifest.assets.artwork;
    if (manifest.assets.labelArtwork) compact.la = manifest.assets.labelArtwork;
    if (manifest.assets.overlay) compact.ov = manifest.assets.overlay;
    if (manifest.backgroundColor !== DEFAULT_BG) compact.bg = manifest.backgroundColor;
    if (manifest.recordColor !== DEFAULT_RECORD) compact.fg = manifest.recordColor;
    if (manifest.label.background !== DEFAULT_LABEL) compact.lb = manifest.label.background;
    if (manifest.label.textColor !== DEFAULT_TEXT) compact.tc = manifest.label.textColor;
    if (manifest.label.sideColor !== "#ff2c2c") compact.sc = manifest.label.sideColor;
    if (Number(manifest.artworkOpacity) !== 0.45) compact.ao = Number(manifest.artworkOpacity);
    if (Number(manifest.overlayOpacity) !== 0.35) compact.oo = Number(manifest.overlayOpacity);
    if (Number(manifest.label.artworkOpacity) !== 0.4) compact.lao = Number(manifest.label.artworkOpacity);
    if (Number(manifest.label.radius) !== Number(manifest.geometry.labelRadius)) compact.lr = Number(manifest.label.radius);
    if (manifest.cropScanlines) compact.cs = true;
    if (manifest.bValue) compact.b = Number(manifest.bValue);
    if (manifest.payload.byteLength) compact.bl = Number(manifest.payload.byteLength);
    if (manifest.payload.pixelCount) compact.pc = Number(manifest.payload.pixelCount);
    if (manifest.label.ordinalNumber) compact.on = manifest.label.ordinalNumber;
    if (manifest.label.satoshi) compact.sat = manifest.label.satoshi;
    if (manifest.label.block) compact.bn = manifest.label.block;
    if (manifest.label.minedBy) compact.mb = manifest.label.minedBy;
    const g = geometryOverrides(manifest);
    if (hasKeys(g)) compact.g = g;
    compact.l = [
      manifest.label.title || "",
      manifest.label.artist || "",
      manifest.label.subtitle || "",
      manifest.label.side || "",
      manifest.label.duration || "",
      manifest.label.publisherYear || "",
      manifest.label.runoutText || "",
      manifest.label.rightText || "",
      manifest.label.leftText || "",
    ];
    while (compact.l.length && compact.l[compact.l.length - 1] === "") compact.l.pop();
    return compact;
  }

  async function loadManifest(value, options = {}) {
    if (typeof value !== "string") return normalizeManifest(value);
    const trimmed = value.trim();
    if (trimmed.startsWith("{")) return normalizeManifest(trimmed);
    const response = await fetch(ordinalPath(trimmed, options.baseUrl), { cache: "force-cache" });
    if (!response.ok) throw new Error(`Failed to load Bitneedle ordinal JSON: HTTP ${response.status}`);
    return normalizeManifest(await response.json());
  }

  async function renderRecord(manifestInput, {
    canvas,
    baseUrl = "",
    mirrorFallback = true,
  } = {}) {
    const manifest = await loadManifest(manifestInput, { baseUrl });
    const targetCanvas = canvas || makeCanvas(manifest.width, manifest.height).canvas;
    targetCanvas.width = manifest.width;
    targetCanvas.height = manifest.height;
    const context = targetCanvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("Canvas 2D is unavailable.");
    const masterLabel = manifest.labelStyle === "master";

    const [rgbImage, artwork, labelArtwork, overlay] = await Promise.all([
      loadImage(manifest.assets.rgbBlock, { baseUrl, mirrorFallback }),
      loadImage(manifest.assets.artwork, { baseUrl, mirrorFallback }),
      loadImage(manifest.assets.labelArtwork || (masterLabel ? "" : manifest.assets.artwork), { baseUrl, mirrorFallback }),
      loadImage(manifest.assets.overlay, { baseUrl, mirrorFallback }),
    ]);

    const rgbImageData = imageToImageData(rgbImage, { cropScanlines: manifest.cropScanlines });
    const trackPixels = readRgbBlockPixels(rgbImageData, manifest.payload);
    const trackPixelCount = Math.floor(trackPixels.length / 4);
    const bFit = manifest.bValue
      ? { bValue: Number(manifest.bValue), count: null }
      : fitBValue(trackPixelCount, {
        width: manifest.width,
        height: manifest.height,
        geometry: manifest.geometry,
      });
    const bValue = Number(bFit.bValue);
    const spiralIndices = buildPayloadSpiralIndices({
      width: manifest.width,
      height: manifest.height,
      bValue,
      geometry: manifest.geometry,
    });

    context.clearRect(0, 0, manifest.width, manifest.height);
    context.fillStyle = masterLabel ? manifest.label.background || manifest.labelColor : manifest.backgroundColor;
    context.fillRect(0, 0, manifest.width, manifest.height);

    const cx = manifest.width / 2;
    const cy = manifest.height / 2;
    drawCircle(context, cx, cy, manifest.geometry.outerRadius, manifest.recordColor);

    if (artwork && !masterLabel) {
      context.save();
      context.beginPath();
      context.arc(cx, cy, manifest.geometry.outerRadius, 0, Math.PI * 2);
      context.clip();
      context.globalCompositeOperation = "screen";
      context.globalAlpha = clamp(manifest.artworkOpacity, 0, 1, 0.45);
      drawCenteredImage(context, artwork, cx - manifest.geometry.outerRadius, cy - manifest.geometry.outerRadius, manifest.geometry.outerRadius * 2, manifest.geometry.outerRadius * 2);
      context.restore();
    }

    const image = context.getImageData(0, 0, manifest.width, manifest.height);
    const data = image.data;
    const max = Math.min(trackPixelCount, spiralIndices.length);
    for (let i = 0; i < max; i += 1) {
      const pixelIndex = spiralIndices[i] * 4;
      const source = i * 4;
      data[pixelIndex] = trackPixels[source];
      data[pixelIndex + 1] = trackPixels[source + 1];
      data[pixelIndex + 2] = trackPixels[source + 2];
      data[pixelIndex + 3] = trackPixels[source + 3] || 255;
    }
    context.putImageData(image, 0, 0);

    if (masterLabel) {
      drawMasterLabel(context, manifest, { artwork, labelArtwork });
    } else {
      drawLabel(context, manifest, { labelArtwork });
    }

    if (overlay) {
      context.save();
      context.globalAlpha = clamp(manifest.overlayOpacity, 0, 1, 0.35);
      context.globalCompositeOperation = "color-dodge";
      drawCenteredImage(context, overlay, 0, 0, manifest.width, manifest.height);
      context.restore();
    }

    return {
      canvas: targetCanvas,
      context,
      manifest,
      stats: {
        bValue,
        rgbBlockWidth: rgbImageData.width,
        rgbBlockHeight: rgbImageData.height,
        trackPixelCount,
        spiralPixelCount: spiralIndices.length,
        pixelsWritten: max,
        overflowPixels: Math.max(0, trackPixelCount - spiralIndices.length),
        unusedSpiralPixels: Math.max(0, spiralIndices.length - trackPixelCount),
      },
    };
  }

  function canvasToPngBlob(canvas) {
    if (!canvas) return Promise.reject(new Error("A rendered canvas is required for PNG export."));
    if (canvas.toBlob) {
      return new Promise((resolve, reject) => {
        try {
          canvas.toBlob((blob) => {
            if (blob) resolve(blob);
            else reject(new Error("Browser failed to encode the canvas as PNG."));
          }, "image/png");
        } catch (error) {
          reject(error);
        }
      });
    }
    const dataUrl = canvas.toDataURL("image/png");
    const [header, data] = dataUrl.split(",");
    const mime = /data:([^;]+)/.exec(header)?.[1] || "image/png";
    const binary = atob(data);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return Promise.resolve(new Blob([bytes], { type: mime }));
  }

  function pngFilename(manifest) {
    const text = String(manifest?.label?.title || "bitneedle-record")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "");
    return `${text || "bitneedle-record"}.png`;
  }

  async function downloadPng(target, filename) {
    const canvas = target?.canvas || target;
    const manifest = target?.manifest;
    const blob = await canvasToPngBlob(canvas);
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = filename || pngFilename(manifest);
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    return blob;
  }

  async function bootstrap({
    manifest,
    manifestElementId = "bitneedle-ordinal-record",
    canvasId = "bitneedle-record",
    downloadId = "bitneedle-download",
    downloadFilename,
    imageId,
    baseUrl = "",
  } = {}) {
    let source = manifest;
    if (!source) {
      const element = document.getElementById(manifestElementId);
      if (!element) throw new Error(`Missing Bitneedle ordinal manifest element: #${manifestElementId}`);
      source = element.textContent || "";
    }
    const canvas = document.getElementById(canvasId) || makeCanvas().canvas;
    if (!canvas.parentNode) document.body.appendChild(canvas);
    const result = await renderRecord(source, { canvas, baseUrl });
    if (imageId) {
      const image = document.getElementById(imageId);
      if (image) image.src = result.canvas.toDataURL("image/png");
    }
    if (downloadId) {
      const download = document.getElementById(downloadId);
      if (download) {
        download.removeAttribute("disabled");
        download.addEventListener("click", (event) => {
          event.preventDefault();
          downloadPng(result, downloadFilename).catch((error) => {
            download.setAttribute("data-error", error.message || String(error));
            globalScope.dispatchEvent?.(new CustomEvent("bitneedle:ordinal-error", { detail: error }));
          });
        });
      }
    }
    globalScope.dispatchEvent?.(new CustomEvent("bitneedle:ordinal-rendered", { detail: result.stats }));
    return result;
  }

  globalScope.BitneedleOrdinal = Object.freeze({
    PROFILE_GEOMETRY,
    buildPayloadSpiralIndices,
    bootstrap,
    canvasToPngBlob,
    compactManifest,
    downloadPng,
    fitBValue,
    loadImage,
    loadManifest,
    normalizeContentPath,
    normalizeManifest,
    ordinalPath,
    parseManifest,
    renderRecord,
    traceRecordSpiral,
    validateManifest,
  });
})(globalThis);
