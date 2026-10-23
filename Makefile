.PHONY: help sync dist-linux dist-macos dist-windows test lint typecheck coverage clean changelog release

UV ?= uv
VERSION ?=

help:
	@echo "FrameCover build targets"
	@echo ""
	@echo "  sync         Install / sync all dependencies via uv"
	@echo "  dist-linux   Build a Linux single-file executable  (run on Linux)"
	@echo "  dist-macos   Build a macOS .app bundle             (run on macOS)"
	@echo "  dist-windows Build a Windows .exe                  (run on Windows)"
	@echo "  NOTE: PyInstaller cannot cross-compile. Each dist target must be"
	@echo "        run on its native OS. Use GitHub Actions for CI builds."
	@echo ""
	@echo "  test         Run unit and integration tests"
	@echo "  lint         Run ruff linter"
	@echo "  typecheck    Run ty type check"
	@echo "  coverage     Run tests with >=80% coverage gate"
	@echo "  changelog    Generate CHANGELOG.md from Conventional Commits"
	@echo "  release      Commit and tag a release (VERSION=X.Y.Z, or auto)"
	@echo "               Does not push; push the branch and tag to publish"
	@echo "  clean        Remove build artefacts"

# ── Environment ───────────────────────────────────────────────────────────────

sync:
	$(UV) sync --extra dev

changelog:
	$(UV) run git-changelog --output CHANGELOG.md

release:
	$(UV) run python scripts/release.py create --version "$(VERSION)" --uv "$(UV)"

# ── Distribution ─────────────────────────────────────────────────────────────

dist-linux:
	$(UV) run pyinstaller pyinstaller.spec \
		--distpath dist/linux \
		--workpath build/linux \
		--noconfirm

dist-macos:
	$(UV) run pyinstaller pyinstaller.spec \
		--distpath dist/macos \
		--workpath build/macos \
		--noconfirm

dist-windows:
	$(UV) run pyinstaller pyinstaller.spec \
		--distpath dist/windows \
		--workpath build/windows \
		--noconfirm

# ── Quality gates ─────────────────────────────────────────────────────────────

test:
	$(UV) run pytest tests/unit/ tests/integration/ -q

lint:
	$(UV) run ruff check src/ tests/ scripts/

typecheck:
	$(UV) run ty check

coverage:
	$(UV) run pytest --cov=framecover \
		--cov-report=term-missing \
		--cov-fail-under=80 \
		tests/unit/ tests/integration/

# ── Housekeeping ──────────────────────────────────────────────────────────────

clean:
	rm -rf build/ dist/ __pycache__ .pytest_cache \
		src/framecover/__pycache__ \
		*.egg-info
