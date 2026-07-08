/* tslint:disable */
/* eslint-disable */

export class WasmLabelThumbnail {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    bytes(): Uint8Array;
    mime(): string;
}

export class WasmPayloadDecodeResult {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    chunkStreamBytes(): Uint8Array;
    metadataJson(): string;
    payloadBytes(): Uint8Array;
    /**
     * JSON array of `{afterByteOffset, sampleCount}` spans describing where,
     * in `payloadBytes`, GAP-container silence belongs. `payloadBytes` itself
     * excludes GAP entries entirely (they carry no real codec data — see
     * `PAYLOAD_CONTAINER_GAP`), so the EnCodec decoder never sees them; the
     * player splices `sampleCount` zero-filled PCM samples after decoding
     * the byte at `afterByteOffset` instead. Empty array (`"[]"`) when the
     * record has no GAP entries (the common case).
     */
    silenceMapJson(): string;
}

export function buildEcdcProgrammeJson(options_json: string): string;

export function buildFixedContextSegmentPlanJson(options_json: string): string;

export function buildMultiTrackSegmentBudgetJson(options_json: string): string;

export function cacheEncryptionRecordBindingHashHex(descriptor_json: string): string;

export function decodeRecordDescriptorHeaderJson(png_bytes: Uint8Array, record_profile?: string | null): string;

export function decodeRecordMetadataJson(png_bytes: Uint8Array): string;

export function decodeRecordPngSidecar(png_bytes: Uint8Array, record_profile?: string | null): Uint8Array;

export function decodeRecordPngSidecarItemsJson(png_bytes: Uint8Array, record_profile?: string | null): string;

export function decodeRecordPngSidecarJson(png_bytes: Uint8Array, record_profile?: string | null): string;

export function decodeRecordPngToPayload(png_bytes: Uint8Array): WasmPayloadDecodeResult;

export function decodeRecordPngToPayloadForProfile(png_bytes: Uint8Array, record_profile: string): WasmPayloadDecodeResult;

export function decodeRecordPngToPayloadForProfileWithLength(png_bytes: Uint8Array, record_profile: string, byte_length: number): WasmPayloadDecodeResult;

export function decodeRecordPngToPayloadForProfileWithTurns(png_bytes: Uint8Array, record_profile: string, _visible_turns: number): WasmPayloadDecodeResult;

export function decodeRecordPngToPayloadForProfileWithTurnsAndLength(png_bytes: Uint8Array, record_profile: string, _visible_turns: number, byte_length: number): WasmPayloadDecodeResult;

export function decodeRecordPngToPayloadWithLength(png_bytes: Uint8Array, byte_length: number): WasmPayloadDecodeResult;

/**
 * Pre-decode programme map: exact musical/GAP sample boundaries and total
 * duration, recoverable directly from the BRS1 metadata + payload bytes without
 * any neural/PCM decode (plan §8.4, §11.1). `grooveStart`/`grooveEnd` for GAP
 * bands are populated by the renderer's groove-anchor map and are omitted here
 * until that path is wired.
 */
export function decodeRecordProgrammeMapJson(png_bytes: Uint8Array): string;

export function decodeSidecarContainerItemsJson(bts1: Uint8Array): string;

export function decryptCacheEntry(descriptor_json: string, context_json: string, envelope: Uint8Array): Uint8Array;

export function encryptCacheEntry(descriptor_json: string, context_json: string, plaintext: Uint8Array): Uint8Array;

export function extractLabelThumbnail(png_bytes: Uint8Array, record_profile?: string | null): WasmLabelThumbnail;

export function inferRecordProfileFromPng(png_bytes: Uint8Array): string;

export function initPanicHook(): void;

export function recordDescriptorMagic(): string;

export function recordPngToRgbColorBlockPng(png_bytes: Uint8Array, record_profile?: string | null): Uint8Array;

export function recordWasmBuildInfoJson(): string;

export function validateRecordHeaderJson(png_bytes: Uint8Array, record_profile?: string | null): string;

export function validateSidecarContainer(bts1: Uint8Array): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_wasmlabelthumbnail_free: (a: number, b: number) => void;
    readonly __wbg_wasmpayloaddecoderesult_free: (a: number, b: number) => void;
    readonly buildEcdcProgrammeJson: (a: number, b: number) => [number, number, number, number];
    readonly buildFixedContextSegmentPlanJson: (a: number, b: number) => [number, number, number, number];
    readonly buildMultiTrackSegmentBudgetJson: (a: number, b: number) => [number, number, number, number];
    readonly cacheEncryptionRecordBindingHashHex: (a: number, b: number) => [number, number, number, number];
    readonly decodeRecordDescriptorHeaderJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly decodeRecordMetadataJson: (a: number, b: number) => [number, number, number, number];
    readonly decodeRecordPngSidecar: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly decodeRecordPngSidecarItemsJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly decodeRecordPngSidecarJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly decodeRecordPngToPayload: (a: number, b: number) => [number, number, number];
    readonly decodeRecordPngToPayloadForProfile: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly decodeRecordPngToPayloadForProfileWithLength: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly decodeRecordPngToPayloadForProfileWithTurns: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly decodeRecordPngToPayloadForProfileWithTurnsAndLength: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly decodeRecordPngToPayloadWithLength: (a: number, b: number, c: number) => [number, number, number];
    readonly decodeRecordProgrammeMapJson: (a: number, b: number) => [number, number, number, number];
    readonly decodeSidecarContainerItemsJson: (a: number, b: number) => [number, number, number, number];
    readonly decryptCacheEntry: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly encryptCacheEntry: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly extractLabelThumbnail: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly inferRecordProfileFromPng: (a: number, b: number) => [number, number, number, number];
    readonly recordDescriptorMagic: () => [number, number];
    readonly recordPngToRgbColorBlockPng: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly recordWasmBuildInfoJson: () => [number, number];
    readonly validateRecordHeaderJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly validateSidecarContainer: (a: number, b: number) => [number, number, number, number];
    readonly wasmlabelthumbnail_bytes: (a: number) => [number, number];
    readonly wasmlabelthumbnail_mime: (a: number) => [number, number];
    readonly wasmpayloaddecoderesult_chunkStreamBytes: (a: number) => [number, number];
    readonly wasmpayloaddecoderesult_metadataJson: (a: number) => [number, number];
    readonly wasmpayloaddecoderesult_payloadBytes: (a: number) => [number, number];
    readonly wasmpayloaddecoderesult_silenceMapJson: (a: number) => [number, number];
    readonly initPanicHook: () => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
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
