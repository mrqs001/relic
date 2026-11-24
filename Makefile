.PHONY: build test run fixtures

build:
	npm --prefix web ci
	npm --prefix web run build
	cargo build --manifest-path app/Cargo.toml --locked --release
	if [ -f private-sources/Cargo.toml ]; then cargo build --manifest-path private-sources/Cargo.toml --locked --release; fi

test:
	cargo test --manifest-path app/Cargo.toml --locked
	if [ -f private-sources/Cargo.toml ]; then cargo test --manifest-path private-sources/Cargo.toml --workspace --locked; fi
	cargo test --manifest-path integrations/telegram/Cargo.toml --locked
	npm --prefix web run typecheck
	node validation/check_handoff.mjs
	node validation/check_natural_earth.mjs
	node validation/check_evidence_media.mjs
	node validation/check_routes.mjs
	node validation/check_time_windows.mjs
	node validation/check_reading_contract.mjs

run:
	./relic

fixtures:
	node validation/generate_workload.mjs
