WASM_PACK ?= wasm-pack
WASM_TARGET ?= web

.PHONY: record-wasm record-cut-wasm bitneedle-wasm

record-wasm:
	$(WASM_PACK) build record-wasm --target $(WASM_TARGET) --out-dir pkg

record-cut-wasm:
	$(WASM_PACK) build record-cut-wasm --target $(WASM_TARGET) --out-dir pkg

wasm: record-wasm record-cut-wasm
