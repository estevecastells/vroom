.PHONY: build test lint fmt app install record-check ci clean help

build:            ## Build the CLI (release)
	cargo build --release --locked

test:             ## Rust tests (incl. a fake S3) + Swift protocol checks + app translations
	cargo test --locked
	cd app && swift run -c debug KitChecks
	swift scripts/check-strings.swift

lint:             ## Formatting and lints
	cargo fmt --all --check
	cargo clippy --locked --all-targets -- -D warnings

fmt:
	cargo fmt --all

app:              ## Build build/Vroom.app
	scripts/build-app.sh

install: app      ## Copy Vroom.app to /Applications
	rm -rf /Applications/Vroom.app
	cp -R build/Vroom.app /Applications/
	@echo "Open /Applications/Vroom.app"

record-check:     ## Record 4 s of the main display through the capture pipeline
	cd app && swift run RecordCheck /tmp/vroom-check.mp4 4

ci: lint test app ## Everything a change must pass

clean:
	cargo clean
	rm -rf build app/.build

help:
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-14s %s\n", $$1, $$2}'
