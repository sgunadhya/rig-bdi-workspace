.PHONY: build-ui run-app run-server demo test test-core test-server test-ui check check-ui-tauri sample-webhook clean-db

build-ui:
	cd incident-ui && NO_COLOR=false trunk build --release --dist ../ui --public-url ./

run-app: build-ui
	cargo run -p src-tauri --features tauri-app

run-server:
	cargo run -p agent-server

demo:
	cargo run -p src-tauri -- --demo

test: test-core test-server test-ui

test-core:
	cargo test -p agent-core

test-server:
	cargo test -p agent-server

test-ui:
	cargo test -p src-tauri

check:
	cargo check --workspace

check-ui-tauri:
	cargo check -p src-tauri --features tauri-app

sample-webhook:
	curl -X POST http://127.0.0.1:8080/webhook/generic \
		-H 'Content-Type: application/json' \
		-d '{"id":"inc-sample","title":"Pod crashlooping","severity":"high","tags":["crashloop"]}'

clean-db:
	rm -f incidents.db
