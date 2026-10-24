# Thin convenience wrapper: every target delegates to cargo / `cargo xtask`,
# which works the same without make (e.g. on a plain Windows shell).

.PHONY: help ffmpeg build run test lint fmt changelog release dist clean

CARGO ?= cargo
VERSION ?=

ifeq ($(OS),Windows_NT)
  PLATFORM ?= windows
else ifeq ($(shell uname -s),Darwin)
  PLATFORM ?= macos
else
  PLATFORM ?= linux
endif

help:
	@echo "FrameCover build targets (see 'cargo xtask help' for details)"
	@echo ""
	@echo "  ffmpeg     Download the pinned FFmpeg build for this OS into vendor/ffmpeg/"
	@echo "  build      Build the app (debug)"
	@echo "  run        Run the app (debug)"
	@echo "  test       Run all workspace tests"
	@echo "  lint       cargo fmt --check + clippy -D warnings"
	@echo "  fmt        Format the code"
	@echo "  changelog  Regenerate CHANGELOG.md (git-cliff)"
	@echo "  release    Commit and tag a release (VERSION=X.Y.Z, or auto); never pushes"
	@echo "  dist       Build release-assets/framecover-<version>-$(PLATFORM).* for this OS"
	@echo "  clean      Remove build artefacts"

ffmpeg:
	$(CARGO) xtask fetch-ffmpeg

build:
	$(CARGO) build -p framecover

run:
	$(CARGO) run -p framecover

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all

changelog:
	$(CARGO) xtask changelog

release:
	$(CARGO) xtask release create $(if $(VERSION),--version $(VERSION),)

dist: ffmpeg
	$(CARGO) xtask package $(PLATFORM)

clean:
	$(CARGO) clean
	rm -rf release-assets
