.PHONY: build test lint fmt check deploy

build:
	cargo build --workspace

test:
	cargo test --workspace

lint:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

check: fmt lint test
	@echo "All checks passed"

deploy:
	cd ~/deploy/krolik-server && docker compose build --no-cache ox-codes && docker compose up -d --no-deps --force-recreate ox-codes
