WASM_PACK ?= wasm-pack
WASM_TARGET ?= web
RECORD_WASM_INITIAL_MEMORY ?= 16777216
RECORD_AUTHOR_WASM_INITIAL_MEMORY ?= 67108864

.PHONY: record-wasm record-cut-wasm bitneedle-wasm

record-wasm:
	CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="$(CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS) -C link-arg=--initial-memory=$(RECORD_WASM_INITIAL_MEMORY)" \
		$(WASM_PACK) build record-wasm --target $(WASM_TARGET) --out-dir pkg

record-cut-wasm:
	CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="$(CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS) -C link-arg=--initial-memory=$(RECORD_AUTHOR_WASM_INITIAL_MEMORY)" \
		$(WASM_PACK) build record-cut-wasm --target $(WASM_TARGET) --out-dir pkg

wasm: record-wasm record-cut-wasm
