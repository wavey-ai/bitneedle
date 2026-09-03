/* tslint:disable */
/* eslint-disable */

export class WasmRenderResult {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    headerJson(): string;
    payloadJson(): string;
    pngBytes(): Uint8Array;
}

export function buildBitneedlePackage(brd1_bytes: Uint8Array, brs1_bytes: Uint8Array, bsc1_bytes: Uint8Array): Uint8Array;

export function buildPackageCoverItemJson(avif_bytes: Uint8Array): string;

export function buildPackageDisplayHeader(options_json: string): Uint8Array;

export function buildPackageDisplayHeaderItemJson(options_json: string): string;

export function buildPackageMetadataItemsJson(options_json: string): string;

export function buildPackagePhotoItemJson(options_json: string, avif_bytes: Uint8Array): string;

export function buildPackageSidecarRenderOptionsJson(options_json: string): string;

export function buildSidecarContainer(items_json: string): Uint8Array;

export function estimateRecordPngSidecarCapacityJson(png_bytes: Uint8Array, record_profile?: string | null): string;

export function extractBitneedlePackageSection(package_bytes: Uint8Array, section_name: string): Uint8Array;

export function inspectBitneedlePackageJson(package_bytes: Uint8Array): string;

export function normalizeRecordProfileName(record_profile: string): string;

export function packageFitBudgetJson(options_json: string): string;

export function packagePreservedPatternItemsJson(decoded_json: string): string;

export function packageQuantizerSearchPlanJson(options_json: string): string;

export function patternizeRecordPngExplore(png_bytes: Uint8Array, options_json: string, record_profile?: string | null): WasmRenderResult;

export function pressCertainLpRecordFormatJson(duration_seconds: number, current_profile: string, current_quality: string): string;

export function pressRecordDurationEstimateJson(record_profile: string, quality: string): string;

export function pressRecordDurationHintJson(record_profile: string, quality: string): string;

export function pressRecordFormatRecommendationJson(options_json: string): string;

export function recordLabelProfileSpecJson(record_profile: string): string;

export function recordLabelProfileSpecsJson(): string;

export function recordProfileSpecJson(record_profile: string): string;

export function recordProfileSpecsJson(): string;

export function renderEmptyGrooveRecordToPng(record_profile: string, red: number, green: number, blue: number): WasmRenderResult;

export function renderPayloadCodesToPng(codes: Uint8Array, code_format: string, record_profile: string, duration_seconds: number, render_options_json: string): WasmRenderResult;

export function renderPayloadContainerToPng(payload: Uint8Array, payload_container: string, payload_codec: string, code_format: string, record_profile: string, duration_seconds: number, render_options_json: string): WasmRenderResult;

export function renderPayloadEntriesToPng(payload_entries: any, payload_container: string, code_format: string, record_profile: string, duration_seconds: number, render_options_json: string): WasmRenderResult;

/**
 * Render headerless payload entries that all share one `PayloadDescriptor`
 * (provided as JSON). The descriptor is stored once in the BRS1 metadata and
 * every entry references descriptor index 0; the RGB groove stores only the
 * headerless codec payload bytes.
 */
export function renderPayloadEntriesWithDescriptorToPng(payload_entries: any, payload_descriptor_json: string, code_format: string, record_profile: string, duration_seconds: number, render_options_json: string): WasmRenderResult;

/**
 * Render headerless payload entries that reference one of several shared
 * `PayloadDescriptor`s (e.g. one ECDC descriptor for song audio plus one GAP
 * descriptor for inter-track silence placeholders), with an explicit track
 * listing. Generalizes `renderPayloadEntriesWithDescriptorToPng` (which
 * always assumes a single descriptor and one track spanning every entry) for
 * callers that need multiple descriptors and/or more than one track.
 * Canonical record-authoring entry point (plan §8.1). Takes the headerless
 * per-revolution ECDC payload entries plus a programme JSON describing the
 * musical tracks (title, the ECDC `payloadIndexes` they cover, and the
 * `gapAfterSeconds` of trailing silence). All GAP timing, sizing, seeds, and
 * canonical GAP1 bytes are derived in Rust; JavaScript never builds GAP
 * metadata or bytes.
 */
export function renderRecordProgrammeToPng(ecdc_payload_entries: any, programme_json: string, code_format: string, record_profile: string, render_options_json: string): WasmRenderResult;

export function resolvePackageBestFitCacheKeyJson(options_json: string): string;

export function resolvePackageImageEncodeCacheKeyJson(options_json: string): string;

export function resolveRecordLabelCutoutStyleJson(record_profile: string, style_json: string): string;

export function rewriteRecordPng(png_bytes: Uint8Array, render_options_json: string, record_profile?: string | null): WasmRenderResult;

export function visibleSpiralTurns(record_profile: string, b_value: number): number;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmrenderresult_free: (a: number, b: number) => void;
    readonly buildBitneedlePackage: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly buildPackageCoverItemJson: (a: number, b: number) => [number, number];
    readonly buildPackageDisplayHeader: (a: number, b: number) => [number, number, number, number];
    readonly buildPackageDisplayHeaderItemJson: (a: number, b: number) => [number, number, number, number];
    readonly buildPackageMetadataItemsJson: (a: number, b: number) => [number, number, number, number];
    readonly buildPackagePhotoItemJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly buildPackageSidecarRenderOptionsJson: (a: number, b: number) => [number, number, number, number];
    readonly buildSidecarContainer: (a: number, b: number) => [number, number, number, number];
    readonly estimateRecordPngSidecarCapacityJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly extractBitneedlePackageSection: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly inspectBitneedlePackageJson: (a: number, b: number) => [number, number, number, number];
    readonly normalizeRecordProfileName: (a: number, b: number) => [number, number, number, number];
    readonly packageFitBudgetJson: (a: number, b: number) => [number, number, number, number];
    readonly packagePreservedPatternItemsJson: (a: number, b: number) => [number, number, number, number];
    readonly packageQuantizerSearchPlanJson: (a: number, b: number) => [number, number, number, number];
    readonly patternizeRecordPngExplore: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly pressCertainLpRecordFormatJson: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly pressRecordDurationEstimateJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly pressRecordDurationHintJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly pressRecordFormatRecommendationJson: (a: number, b: number) => [number, number, number, number];
    readonly recordLabelProfileSpecJson: (a: number, b: number) => [number, number, number, number];
    readonly recordLabelProfileSpecsJson: () => [number, number, number, number];
    readonly recordProfileSpecJson: (a: number, b: number) => [number, number, number, number];
    readonly recordProfileSpecsJson: () => [number, number, number, number];
    readonly renderEmptyGrooveRecordToPng: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly renderPayloadCodesToPng: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly renderPayloadContainerToPng: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number) => [number, number, number];
    readonly renderPayloadEntriesToPng: (a: any, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly renderPayloadEntriesWithDescriptorToPng: (a: any, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly renderRecordProgrammeToPng: (a: any, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly resolvePackageBestFitCacheKeyJson: (a: number, b: number) => [number, number, number, number];
    readonly resolvePackageImageEncodeCacheKeyJson: (a: number, b: number) => [number, number, number, number];
    readonly resolveRecordLabelCutoutStyleJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly rewriteRecordPng: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly visibleSpiralTurns: (a: number, b: number, c: number) => [number, number, number];
    readonly wasmrenderresult_headerJson: (a: number) => [number, number];
    readonly wasmrenderresult_payloadJson: (a: number) => [number, number];
    readonly wasmrenderresult_pngBytes: (a: number) => [number, number];
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
