.PHONY: wasm
wasm:
	wasm-pack build record-wasm --target web --out-dir pkg
